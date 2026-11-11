//! 每对端打洞失败记忆表(V3-6 Task 1,spec §4.1):中继三层「并行预备 →
//! armed 记忆 → 探洞回切」的记忆层。传输轮打洞穷尽经
//! [`PunchMemory::note_exhaust`] 记账(armed 阈值 = 1:单次穷尽即 armed,
//! spec §4.1 用户语义「打洞失败后通过中继连上」);armed 轮由 fallback 单缝
//! 消费(Task 6:发起方跳直连直进中继、响应方跳互拨等推窗);任一次直连
//! 成功(打洞成/暖池命中/暖插入)经 [`PunchMemory::note_direct_success`]
//! 清账;活跃中继轮登记是探洞豁免与回切轮换的判据(Task 9),与失败记忆
//! 同表分列。
//!
//! **TTL 惰性衰减**:15min 无新穷尽即衰减归零(armed 随之失效、轮换标记
//! 复位),判定挂读路径(is_armed / take_rotation_shot 与再计数前),无后台
//! 任务——warmer 冷却窗盲区的安全阀,重 armed 需一次新鲜穷尽(spec §4.1)。
//!
//! **armed 代际 = 计数值**:代际号随穷尽累进;一次代际只轮换一发
//! ([`PunchMemory::take_rotation_shot`],防振荡 spec §4.4——回切后直连再死
//! 须重走穷尽再 armed,绝不乒乓)。发过标记随撤 armed / TTL 衰减 / 新代际
//! 复位;衰减归零后再计的代际号与旧代际同值(1),复位语义保证同号不串。
//!
//! **中继轮登记累积**:同一 peer 可同时有发送轮与接收轮两条登记(各自
//! register,连接克隆句柄入库);轮终局 `clear` 全清、回切轮换经 `drain`
//! 取走全部句柄显式 close。登记的生命周期**属于轮本身**——直连成功与 TTL
//! 衰减都不动它(收口由轮两端自担)。
//!
//! 锁纪律:XferSlots 同款 std Mutex(不跨 await,各方法锁内即返);TTL 与
//! 探洞下限是 env 旋钮族(`MTDSS_TEST_*`,warm_debounce_from 同款注入缝)。
//! 消费面在 Task 6 / Task 9 接线,本任务只落纯模块 + state 接线(无调用
//! 点,dead_code 按 account.rs `peer_pk_from` 先例注记,单测先行直驱)。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// armed 记忆 TTL 锚(spec §11 裁量 3):15min 无新穷尽即衰减归零。常量是
/// 文档锚;进程内生效值经 [`punch_mem_ttl`](env 注入缝)读定。
pub(crate) const PUNCH_MEM_TTL: Duration = Duration::from_secs(15 * 60);

/// 活跃中继轮期间的探洞节奏下限锚(spec §11 裁量 4):30s/发,豁免 warmer
/// 冷却窗。常量是文档锚;进程内生效值经 [`probe_floor_val`](env 注入缝)读定。
pub(crate) const PROBE_FLOOR: Duration = Duration::from_secs(30);

/// TTL 的进程内生效值(env 注入缝,`MTDSS_TEST_*` 家族口径,OnceLock 一次
/// 读定):`MTDSS_TEST_PUNCH_MEM_TTL_MS` = 毫秒值。缺失 → 钉死默认;非法
/// (非数字/空串)→ warn 保持默认(warm_debounce_from 同款房风)。
fn punch_mem_ttl() -> Duration {
    static CACHE: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| punch_mem_ttl_from(|k| std::env::var(k).ok()))
}

/// TTL 映射纯函数(`get` 注入缝,单测同款房风):见 [`punch_mem_ttl`]。
fn punch_mem_ttl_from(get: impl Fn(&str) -> Option<String>) -> Duration {
    match get("MTDSS_TEST_PUNCH_MEM_TTL_MS") {
        None => PUNCH_MEM_TTL,
        Some(raw) => match raw.parse::<u64>() {
            Ok(ms) => Duration::from_millis(ms),
            Err(_) => {
                tracing::warn!("ignored invalid MTDSS_TEST_PUNCH_MEM_TTL_MS={raw:?} (keep default)");
                PUNCH_MEM_TTL
            }
        },
    }
}

/// 探洞下限的进程内生效值(env 注入缝,同 [`punch_mem_ttl`] 口径):
/// `MTDSS_TEST_PROBE_FLOOR_MS` = 毫秒值。
fn probe_floor_val() -> Duration {
    static CACHE: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| probe_floor_from(|k| std::env::var(k).ok()))
}

/// 探洞下限映射纯函数(`get` 注入缝,单测同款房风):见 [`probe_floor_val`]。
fn probe_floor_from(get: impl Fn(&str) -> Option<String>) -> Duration {
    match get("MTDSS_TEST_PROBE_FLOOR_MS") {
        None => PROBE_FLOOR,
        Some(raw) => match raw.parse::<u64>() {
            Ok(ms) => Duration::from_millis(ms),
            Err(_) => {
                tracing::warn!("ignored invalid MTDSS_TEST_PROBE_FLOOR_MS={raw:?} (keep default)");
                PROBE_FLOOR
            }
        },
    }
}

/// 单 peer 条目:失败计数(即 armed 代际号)、末次穷尽时刻(TTL 判据)、
/// 本代际轮换发过标记、活跃中继轮登记(累积)。
struct PunchEntry {
    /// 连续穷尽计数;0 = 未 armed(armed 阈值 = 1)。代际号即本值:衰减归零
    /// 后重新计回 1,发过标记随归零复位,新旧代际同号不串。
    count: u64,
    /// 末次穷尽时刻:TTL 衰减只认它(note_direct_success / 中继登记不刷新
    /// ——「无新穷尽」的「新」专指穷尽)。
    last_exhaust: Instant,
    /// 本 armed 代际的回切轮换是否已发(一发/代际,防振荡)。
    shot_fired: bool,
    /// 活跃中继轮登记(发送轮 + 接收轮可并存,累积);轮终局 clear 全清 /
    /// 回切轮换 drain 取走 close。生命周期属于轮:直连成功与 TTL 衰减都不清。
    relay_conns: Vec<quinn::Connection>,
}

impl PunchEntry {
    fn new() -> Self {
        Self {
            count: 0,
            last_exhaust: Instant::now(),
            shot_fired: false,
            relay_conns: Vec::new(),
        }
    }
}

/// 每对端打洞失败记忆(DaemonState 单实例,随 state 常驻):std Mutex 表,
/// 不跨 await(各方法锁内即返,XferSlots 同款)。语义与生命周期见模块头。
pub(crate) struct PunchMemory {
    /// 表:peer → 条目(衰减后空条目即回收,不留零值残渣)。
    table: Mutex<HashMap<String, PunchEntry>>,
    /// TTL 生效值(Default = env 旋钮读定;for_test 注入短值,免真等)。
    ttl: Duration,
    /// 探洞下限生效值(Default = env 旋钮读定;for_test 注入确定值)。
    probe_floor: Duration,
}

impl Default for PunchMemory {
    fn default() -> Self {
        Self {
            table: Mutex::new(HashMap::new()),
            ttl: punch_mem_ttl(),
            probe_floor: probe_floor_val(),
        }
    }
}

/// TTL 惰性衰减 + 空条目回收(读路径与再计数前的统一闸):末次穷尽距今
/// 超 TTL → 计数归零(armed 撤、发过标记复位);中继轮登记不属失败记忆,
/// 在场则条目保留;衰减后既无计数也无登记 → 条目整体回收。
fn decay_and_prune(table: &mut HashMap<String, PunchEntry>, peer: &str, ttl: Duration) {
    let prune = match table.get_mut(peer) {
        Some(e) => {
            if e.count > 0 && e.last_exhaust.elapsed() > ttl {
                e.count = 0;
                e.shot_fired = false;
            }
            e.count == 0 && e.relay_conns.is_empty()
        }
        None => false,
    };
    if prune {
        table.remove(peer);
    }
}

#[allow(dead_code)] // V3-6 T6 已接消费面(note_exhaust/note_direct_success/is_armed 经 fallback 编排);登记/轮换/探洞面 T9 接线,暂保注记
impl PunchMemory {
    /// 穷尽记账:计数 +1(衰减先于累加——超 TTL 的旧证据先归零,本次即
    /// 新一代),刷新末次穷尽时刻,新代际轮换标记复位;返回代际号(= 新
    /// 计数值)。
    pub(crate) fn note_exhaust(&self, peer: &str) -> u64 {
        let mut table = self.table.lock().unwrap();
        // 衰减先于累加:超 TTL 的旧证据先归零,本次穷尽即新一代(重新计)
        decay_and_prune(&mut table, peer, self.ttl);
        let e = table.entry(peer.to_string()).or_insert_with(PunchEntry::new);
        e.count = e.count.saturating_add(1);
        e.last_exhaust = Instant::now();
        e.shot_fired = false; // 新代际,轮换标记复位
        e.count
    }

    /// 直连成功清账:计数归零、撤 armed、轮换标记复位。中继轮登记不动
    /// (轮生命周期属于轮,见模块头)。
    pub(crate) fn note_direct_success(&self, peer: &str) {
        let mut table = self.table.lock().unwrap();
        if let Some(e) = table.get_mut(peer) {
            e.count = 0;
            e.shot_fired = false;
            // relay_conns 刻意不清:活跃中继轮不因直连成功收口(轮终局
            // clear/drain 自担)。失败记忆已清、无登记 → 条目整体回收。
            if e.relay_conns.is_empty() {
                table.remove(peer);
            }
        }
    }

    /// 该 peer 是否 armed(惰性 TTL:先衰减再判)。
    pub(crate) fn is_armed(&self, peer: &str) -> bool {
        let mut table = self.table.lock().unwrap();
        decay_and_prune(&mut table, peer, self.ttl);
        table.get(peer).is_some_and(|e| e.count >= 1)
    }

    /// 登记一条活跃中继轮(连接克隆句柄入库;同 peer 多条累积)。
    pub(crate) fn register_relay_round(&self, peer: &str, conn: quinn::Connection) {
        let mut table = self.table.lock().unwrap();
        table
            .entry(peer.to_string())
            .or_insert_with(PunchEntry::new)
            .relay_conns
            .push(conn);
    }

    /// 轮终局清账:移除该 peer 的全部中继轮登记。
    pub(crate) fn clear_relay_round(&self, peer: &str) {
        let mut table = self.table.lock().unwrap();
        if let Some(e) = table.get_mut(peer) {
            e.relay_conns.clear();
            if e.count == 0 {
                table.remove(peer);
            }
        }
    }

    /// 是否有在场的中继轮登记。
    pub(crate) fn relay_round_active(&self, peer: &str) -> bool {
        self.table
            .lock()
            .unwrap()
            .get(peer)
            .is_some_and(|e| !e.relay_conns.is_empty())
    }

    /// 取走该 peer 的全部中继轮连接句柄(回切轮换方逐条 close 用;取走
    /// 即清,relay_round_active 随之 false)。
    pub(crate) fn drain_relay_conns(&self, peer: &str) -> Vec<quinn::Connection> {
        let mut table = self.table.lock().unwrap();
        match table.get_mut(peer) {
            Some(e) => {
                let conns = std::mem::take(&mut e.relay_conns);
                if e.count == 0 {
                    table.remove(peer);
                }
                conns
            }
            None => Vec::new(),
        }
    }

    /// 回切轮换一发闸(防振荡):armed 且本代际未发过 → true 并落发过
    /// 标记;未 armed(TTL 过即衰减)或已发过 → false。发过标记随撤 armed
    /// (note_direct_success)与 TTL 衰减复位。
    pub(crate) fn take_rotation_shot(&self, peer: &str) -> bool {
        let mut table = self.table.lock().unwrap();
        decay_and_prune(&mut table, peer, self.ttl);
        match table.get_mut(peer) {
            Some(e) if e.count >= 1 && !e.shot_fired => {
                e.shot_fired = true;
                true
            }
            _ => false,
        }
    }

    /// 探洞节奏下限(warm 退避钳制用,spec §4.4):该 peer 有活跃中继轮
    /// → [`PROBE_FLOOR`](env 旋钮族),否则 `Duration::MAX`(钳制恒不命中,
    /// warmer 既有节奏照旧)。
    pub(crate) fn probe_floor(&self, peer: &str) -> Duration {
        let active = self
            .table
            .lock()
            .unwrap()
            .get(peer)
            .is_some_and(|e| !e.relay_conns.is_empty());
        if active {
            self.probe_floor
        } else {
            Duration::MAX
        }
    }
}

#[cfg(test)]
impl PunchMemory {
    /// 单测构造缝:注入短 TTL 与确定下限,免真等 15min(env 旋钮经 OnceLock
    /// 一次读定,进程内不可按测覆写,故走构造注入)。
    fn for_test(ttl: Duration, probe_floor: Duration) -> Self {
        Self {
            table: Mutex::new(HashMap::new()),
            ttl,
            probe_floor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    /// 短 TTL(2s):断言前相邻语句的调度停顿远小于 TTL;过期面全走
    /// backdate 回拨(无真睡眠,确定性)。
    const TEST_TTL: Duration = Duration::from_secs(2);
    /// 回拨量(3s > TTL,秒级远超测试进程启动耗时,Instant 下溢不可能)。
    const BACKDATE_BEYOND: Duration = Duration::from_secs(3);
    /// 确定探洞下限(与 PROBE_FLOOR 默认 30s 区分,断言注入值本身)。
    const TEST_FLOOR: Duration = Duration::from_secs(7);

    /// 把 peer 的末次穷尽时刻回拨 `backdate` 常量量(免真等 TTL,确定性)。
    fn backdate(mem: &PunchMemory, peer: &str) {
        let mut table = mem.table.lock().unwrap();
        if let Some(e) = table.get_mut(peer) {
            e.last_exhaust = Instant::now()
                .checked_sub(BACKDATE_BEYOND)
                .expect("秒级回拨不可能越过 Instant 下界");
        }
    }

    /// 回环真握手取一对连接句柄(core quic_pinning 房风:B 服务端白名单
    /// A,A pin B 公钥)→(客户端侧, 服务端侧),充作两条独立登记。
    async fn loopback_pair() -> (quinn::Connection, quinn::Connection) {
        use mtdss_core::transport::endpoint::{
            accept_pinned, connect_pinned, make_endpoint, make_server_endpoint,
        };
        let a = mtdss_core::identity::DeviceIdentity::generate();
        let b = mtdss_core::identity::DeviceIdentity::generate();
        let allowed: HashMap<String, ed25519_dalek::VerifyingKey> =
            [(a.device_id.clone(), a.public_key())].into_iter().collect();
        let ep_b =
            make_server_endpoint(SocketAddr::from(([127, 0, 0, 1], 0)), &b, &allowed).unwrap();
        let addr = ep_b.local_addr().unwrap();
        let ep_a = make_endpoint(SocketAddr::from(([127, 0, 0, 1], 0))).unwrap();
        let server = tokio::spawn(async move { accept_pinned(&ep_b, &allowed).await.unwrap().0 });
        let client = connect_pinned(&ep_a, addr, &b.public_key(), &a).await.unwrap();
        (client, server.await.unwrap())
    }

    /// 穷尽记账与 armed(计划 Step 1:单次 exhaust 即 armed):未记账恒不
    /// armed;单次穷尽代际 1 即 armed;再穷现代际累进;他 peer 不受累。
    #[test]
    fn single_exhaust_arms_and_generation_advances() {
        let mem = PunchMemory::for_test(TEST_TTL, TEST_FLOOR);
        assert!(!mem.is_armed("p"), "未穷尽不 armed");
        assert_eq!(mem.note_exhaust("p"), 1, "单次穷尽即 armed,代际 1");
        assert!(mem.is_armed("p"), "armed 阈值 1:单次穷尽即 armed");
        assert_eq!(mem.note_exhaust("p"), 2, "再穷现代际累进(2)");
        assert!(mem.is_armed("p"));
        assert!(!mem.is_armed("q"), "他 peer 不受累");
    }

    /// 直连成功清账(计划 Step 1:direct_success 清):armed 中直连成功即
    /// 撤 armed、轮换不可发;清零后重新计(非旧计数续加);无条目 peer
    /// 幂等无害。
    #[test]
    fn direct_success_clears_and_disarms() {
        let mem = PunchMemory::for_test(TEST_TTL, TEST_FLOOR);
        mem.note_exhaust("p");
        mem.note_exhaust("p"); // 代际 2
        mem.note_direct_success("p");
        assert!(!mem.is_armed("p"), "直连成功即撤 armed");
        assert!(!mem.take_rotation_shot("p"), "撤 armed 后轮换不可发");
        assert_eq!(mem.note_exhaust("p"), 1, "清零后重新计(非 3)");
        mem.note_direct_success("q"); // 无条目:幂等无害
    }

    /// TTL 惰性衰减(计划 Step 1:过期后 is_armed=false 且再 exhaust 重新
    /// 计):回拨出 TTL → 读路径先衰减归零再判;衰减后轮换不可发(标记
    /// 随归零复位);新鲜穷尽重新计回 1(非旧值续加)并重新 armed,新代
    /// 际轮换可发。
    #[test]
    fn ttl_lazy_decay_then_fresh_exhaust_recounts() {
        let mem = PunchMemory::for_test(TEST_TTL, TEST_FLOOR);
        assert_eq!(mem.note_exhaust("p"), 1);
        backdate(&mem, "p");
        assert!(!mem.is_armed("p"), "TTL 过即衰减归零,armed 消失");
        assert!(!mem.take_rotation_shot("p"), "衰减后不可轮换");
        assert_eq!(mem.note_exhaust("p"), 1, "新鲜穷尽重新计(衰减先于累加)");
        assert!(mem.is_armed("p"), "重新 armed");
        assert!(mem.take_rotation_shot("p"), "新代际轮换标记干净,可发");
    }

    /// 回切轮换一发/代际(计划 Step 1:同代际恰一次、清后可再生):armed
    /// 代际首发 true、再发 false;撤 armed 后恒 false;重新 armed(新代
    /// 际)首发再生。
    #[test]
    fn rotation_shot_once_per_generation_rearmable() {
        let mem = PunchMemory::for_test(TEST_TTL, TEST_FLOOR);
        mem.note_exhaust("p"); // 代际 1
        assert!(mem.take_rotation_shot("p"), "armed 代际首发");
        assert!(!mem.take_rotation_shot("p"), "同代际已发,不再发");
        mem.note_direct_success("p");
        assert!(!mem.take_rotation_shot("p"), "未 armed 恒 false");
        assert_eq!(mem.note_exhaust("p"), 1, "重新 armed(新代际)");
        assert!(mem.take_rotation_shot("p"), "清后可再生(新代际首发)");
    }

    /// 中继轮登记生命周期(计划 Step 1):多条累积;drain 取走全部(取走
    /// 即空);drain 后可再登记;clear 全清且幂等;无登记 peer 恒 false。
    #[tokio::test]
    async fn relay_round_lifecycle_register_drain_clear() {
        let mem = PunchMemory::default();
        let (c1, c2) = loopback_pair().await;
        // 首登记即传克隆句柄(生产调用方同款),测试保留本体供再登记
        mem.register_relay_round("p", c1.clone());
        mem.register_relay_round("p", c2);
        assert!(mem.relay_round_active("p"), "两条登记累积在场");
        let drained = mem.drain_relay_conns("p");
        assert_eq!(drained.len(), 2, "drain 取走全部登记");
        assert!(!mem.relay_round_active("p"));
        assert!(mem.drain_relay_conns("p").is_empty(), "取走即空,再 drain 空");
        mem.register_relay_round("p", c1);
        mem.clear_relay_round("p");
        assert!(!mem.relay_round_active("p"), "clear 全清");
        mem.clear_relay_round("p"); // 幂等
        assert!(!mem.relay_round_active("q"), "无登记 peer 恒 false");
    }

    /// 登记生命周期属于轮(模块头契约):直连成功只清失败记忆,不动登记;
    /// TTL 衰减同样只清穷尽证据——两路清账后登记仍在场,轮终局 clear 才
    /// 收口。
    #[tokio::test]
    async fn relay_round_survives_direct_success_and_ttl_decay() {
        let mem = PunchMemory::for_test(TEST_TTL, TEST_FLOOR);
        let (c1, _c2) = loopback_pair().await;
        mem.note_exhaust("p");
        mem.register_relay_round("p", c1);
        mem.note_direct_success("p");
        assert!(!mem.is_armed("p"), "失败记忆已清");
        assert!(
            mem.relay_round_active("p"),
            "直连成功不收中继轮登记(轮生命周期属于轮)"
        );
        backdate(&mem, "p");
        assert!(!mem.is_armed("p"), "衰减只清穷尽证据");
        assert!(mem.relay_round_active("p"), "TTL 衰减同样不动登记");
        mem.clear_relay_round("p");
        assert!(!mem.relay_round_active("p"), "轮终局 clear 才收口");
    }

    /// 探洞下限两态(计划 Step 1):无活跃中继轮 → MAX(钳制不命中,
    /// warmer 既有节奏照旧);登记在场 → 注入下限;drain 后回 MAX。
    #[tokio::test]
    async fn probe_floor_two_states() {
        let mem = PunchMemory::for_test(TEST_TTL, TEST_FLOOR);
        assert_eq!(mem.probe_floor("p"), Duration::MAX, "无活跃中继轮 → 无下限");
        let (c1, _c2) = loopback_pair().await;
        mem.register_relay_round("p", c1);
        assert_eq!(
            mem.probe_floor("p"),
            TEST_FLOOR,
            "活跃中继轮 → 注入下限"
        );
        mem.drain_relay_conns("p");
        assert_eq!(mem.probe_floor("p"), Duration::MAX, "drain 后回到无下限");
    }

    /// TTL env 映射三臂(warm_debounce_env_mapping 单测房风,get 注入缝):
    /// 缺省 = 钉死默认;合法值毫秒换算;非法值(非数字/空串)warn 保持默认。
    #[test]
    fn punch_mem_ttl_env_mapping() {
        let env =
            |v: &'static str| move |k: &str| (k == "MTDSS_TEST_PUNCH_MEM_TTL_MS").then(|| v.to_string());
        assert_eq!(punch_mem_ttl_from(|_| None), PUNCH_MEM_TTL, "缺省 = 钉死默认");
        assert_eq!(
            punch_mem_ttl_from(env("100")),
            Duration::from_millis(100),
            "合法值毫秒换算"
        );
        assert_eq!(punch_mem_ttl_from(env("abc")), PUNCH_MEM_TTL, "非法值 warn 保持默认");
        assert_eq!(punch_mem_ttl_from(env("")), PUNCH_MEM_TTL, "空串非法保持默认");
    }

    /// 探洞下限 env 映射三臂(同上房风)。
    #[test]
    fn probe_floor_env_mapping() {
        let env =
            |v: &'static str| move |k: &str| (k == "MTDSS_TEST_PROBE_FLOOR_MS").then(|| v.to_string());
        assert_eq!(probe_floor_from(|_| None), PROBE_FLOOR, "缺省 = 钉死默认");
        assert_eq!(
            probe_floor_from(env("500")),
            Duration::from_millis(500),
            "合法值毫秒换算"
        );
        assert_eq!(probe_floor_from(env("abc")), PROBE_FLOOR, "非法值 warn 保持默认");
        assert_eq!(probe_floor_from(env("")), PROBE_FLOOR, "空串非法保持默认");
    }

    /// 常量值锚(spec §11 裁量 3/4):改值须过本断言。
    #[test]
    fn tuning_consts_anchor() {
        assert_eq!(PUNCH_MEM_TTL, Duration::from_secs(15 * 60), "TTL 15min");
        assert_eq!(PROBE_FLOOR, Duration::from_secs(30), "探洞下限 30s");
    }
}
