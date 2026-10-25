//! 中继数据面:单 UDP 端口哑转发 + 会话级/全局限速 + 月配额记账 + 杀会话通告。
//! 纯逻辑层(票据/标签/注册表/令牌桶)在 [`ticket`];本模块把它们接上真实
//! socket 与 SQLite,落实裁定 C(配额立即杀/disable 巡检杀/限速热调)、
//! D(逐包鉴权重绑)、H(记账按整数据报长度、双向累计)。

pub mod ticket;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context as _;
use prost::Message as _;
use rusqlite::Connection;

use crate::AppState;
use ticket::{Registry, TokenBucket};

/// 全局带宽上限默认值(kbps),env `MTDSS_RELAY_GLOBAL_KBPS` 可覆盖。
pub const RELAY_GLOBAL_KBPS_DEFAULT: u32 = 2500;
/// 记账刷库字节阈值:1 MiB(spec §9:崩溃丢失记账 ≤1MB 量级)。
const FLUSH_BYTES_THRESHOLD: u64 = 1 << 20;
/// 记账刷库时间阈值:5s(空闲期兜底,与字节阈值任一满足即刷)。
const FLUSH_INTERVAL: Duration = Duration::from_secs(5);
/// 限速/停用巡检周期(裁定 C:变更 60s 内生效)。
const PATROL_INTERVAL: Duration = Duration::from_secs(60);

/// 解析全局带宽上限原始值:未设 → 默认 2500;坏值 → 告警并回退默认;
/// "0" → 0(不限,与会话级 rate_kbps 口径一致)。T5 启动旗标单测直接钉此函数。
pub fn parse_global_kbps(raw: Option<&str>) -> u32 {
    match raw {
        None => RELAY_GLOBAL_KBPS_DEFAULT,
        Some(s) => match s.trim().parse::<u32>() {
            Ok(v) => v,
            Err(_) => {
                tracing::warn!(
                    "MTDSS_RELAY_GLOBAL_KBPS={s:?} 不是合法 kbps,回退默认 {RELAY_GLOBAL_KBPS_DEFAULT}"
                );
                RELAY_GLOBAL_KBPS_DEFAULT
            }
        },
    }
}

/// 读 env 形参版:[`parse_global_kbps`] 的 env 取值包装。
pub fn global_kbps_from_env() -> u32 {
    parse_global_kbps(std::env::var("MTDSS_RELAY_GLOBAL_KBPS").ok().as_deref())
}

/// 解析 `--relay-port` 原始串为中继绑定端口。规则:可解析为非 0 端口
/// (0 不可绑定);默认 8788 由 main 以缺省原始串传入。沿 `parse_stun_ports`
/// 先例提为可测函数,tests/plain.rs 单测钉住。
pub fn parse_relay_port(raw: &str) -> anyhow::Result<u16> {
    let port: u16 = raw
        .trim()
        .parse()
        .map_err(|e| anyhow::anyhow!("--relay-port {raw:?}: {e}"))?;
    anyhow::ensure!(port != 0, "--relay-port {raw:?}: port 0 is not usable");
    Ok(port)
}

/// 校验 `--relay-addr` 通告主机:只收裸 host——main 会追加 `:{port}` 拼通告
/// 地址,值里带 `:`(如 `1.2.3.4:9000`)会拼出畸形通告帧。沿 parse_relay_port
/// 先例提为可测函数。
pub fn parse_relay_host(raw: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !raw.contains(':'),
        "--relay-addr {raw:?}: expected bare host; advertised port comes from --relay-port"
    );
    Ok(())
}

/// 单账号当期配额:内存余量 + 未落库脏字节。
pub struct AccountQuota {
    pub remaining: u64,
    pub dirty: u64,
}

/// 全账号配额台账。周期以 `period` 字段为准(裁定:charge/flush 不读真实钟,
/// 月界由巡检比对 `admin::current_period()` 后滚动;测试可直改字段注入)。
/// 余量 u64::MAX = 不限。
pub struct QuotaLedger {
    pub period: String,
    pub accounts: HashMap<String, AccountQuota>,
}

impl QuotaLedger {
    pub fn new(period: String) -> Self {
        Self { period, accounts: HashMap::new() }
    }

    /// 该账号是否已登记(数据面据以决定是否需要 seed)。
    pub fn known(&self, account_id: &str) -> bool {
        self.accounts.contains_key(account_id)
    }

    /// 幂等预置:按 `relay_entitlement` 认定当期余量;已登记则原样保留。
    pub fn seed(&mut self, conn: &Connection, account_id: &str) -> anyhow::Result<()> {
        if self.known(account_id) {
            return Ok(());
        }
        let (_, _, remaining) = crate::admin::relay_entitlement(conn, account_id)?;
        self.accounts.insert(account_id.to_string(), AccountQuota { remaining, dirty: 0 });
        Ok(())
    }

    /// 记一笔线上字节(整报文含头,双向同价)。返回 false = 本笔超出余量,
    /// 调用方随即杀会话。未登记账号按超限处理(fail-closed,不放大)。
    pub fn charge(&mut self, account_id: &str, bytes: u64) -> bool {
        let Some(q) = self.accounts.get_mut(account_id) else { return false };
        q.dirty = q.dirty.saturating_add(bytes);
        if q.remaining == u64::MAX {
            return true; // 不限:只累脏字节,永不耗尽
        }
        if bytes > q.remaining {
            q.remaining = 0;
            return false; // 超限:本笔照记(已上线),会话交由调用方终止
        }
        q.remaining -= bytes;
        true
    }

    /// 全部账号未落库脏字节之和(1 MiB 阈值判据)。
    pub fn dirty_total(&self) -> u64 {
        self.accounts.values().map(|q| q.dirty).sum()
    }

    /// 脏字节按 `self.period` 字段幂等 upsert 到 relay_usage(同键累加),
    /// 月界滚动后新周期自然从零起算;刷完清脏。
    pub fn flush(&mut self, conn: &Connection) -> anyhow::Result<()> {
        for (acc, q) in self.accounts.iter_mut() {
            if q.dirty == 0 {
                continue;
            }
            conn.execute(
                "INSERT INTO relay_usage(account_id, period, bytes_forwarded) VALUES(?1, ?2, ?3)
                 ON CONFLICT(account_id, period) DO UPDATE SET
                   bytes_forwarded = bytes_forwarded + excluded.bytes_forwarded",
                rusqlite::params![acc, self.period, q.dirty as i64],
            )?;
            q.dirty = 0;
        }
        Ok(())
    }
}

/// 中继数据面共享状态:`spawn_relay` 装配并置入 `AppState.relay`(OnceLock)。
pub struct RelayState {
    pub registry: Mutex<Registry>,
    pub socket: Arc<tokio::net::UdpSocket>,
    /// 标签派生密钥:数据面只按 tag 查表,T4 分配侧用它铸票
    pub secret: [u8; 32],
    /// 全局总闸;rate 0 = 不限(数据面据 TokenBucket::rate_kbps 判定跳过)
    pub global_bucket: Mutex<TokenBucket>,
    pub quota: Mutex<QuotaLedger>,
    /// 对外通告的中继地址(ip:port),T4 填进 RelayAllocated/RelaySession
    pub pub_addr: String,
}

/// 起中继数据面:绑定**通告地址**的 UDP socket(STUN `--bind` 同款语义)、
/// 装配 RelayState 置入 `state.relay`,spawn 转发任务与巡检任务。同步签名
/// (沿 main 装配房风):socket 绑定与 tokio::spawn 都要求运行在 runtime
/// 上下文中。重复调用报错(OnceLock 单次装配)。
///
/// 绑通告地址而非 0.0.0.0 的原因:多网卡主机上 0.0.0.0 的回应源由内核按
/// 出口网卡选择——回程若经另一网卡出,源与通告地址不符,对端 NAT
/// (conntrack 回程命中)与 QUIC 连接校验全部丢弃。绑通告地址后回应源
/// 恒为通告地址,且回应恰好骑在对端入站流的 conntrack 反向元组上(不新
/// 建流);若用 iptables SNAT 改写源则会产生与对端入站流反向元组冲突的
/// 新流,被 netfilter 静默丢弃(nat-matrix 中继用例实测)。通告地址不可
/// 绑(NAT 后部署等)回退 0.0.0.0 并告警。
pub fn spawn_relay(state: Arc<AppState>, bind_port: u16, pub_addr: String) -> anyhow::Result<()> {
    let advertised_ip: Option<std::net::IpAddr> = pub_addr
        .rsplit_once(':')
        .and_then(|(ip, _)| ip.parse().ok());
    let (bind_ip, bind_res) = match advertised_ip {
        Some(ip) => (ip, std::net::UdpSocket::bind((ip, bind_port))),
        None => ("0.0.0.0".parse().unwrap(), std::net::UdpSocket::bind(("0.0.0.0", bind_port))),
    };
    let std_sock = match bind_res {
        Ok(s) => s,
        Err(e) if !bind_ip.is_unspecified() => {
            tracing::warn!("relay udp bind {bind_ip}:{bind_port} 失败({e}),回退 0.0.0.0");
            std::net::UdpSocket::bind(("0.0.0.0", bind_port))
                .with_context(|| format!("relay udp bind 0.0.0.0:{bind_port}"))?
        }
        Err(e) => {
            return Err(anyhow::anyhow!(e)
                .context(format!("relay udp bind {bind_ip}:{bind_port}")))
        }
    };
    std_sock
        .set_nonblocking(true)
        .context("relay udp set_nonblocking")?;
    let socket =
        Arc::new(tokio::net::UdpSocket::from_std(std_sock).context("relay udp register tokio")?);
    let global_kbps = global_kbps_from_env();
    tracing::info!(
        "relay data plane on udp {bind_ip}:{bind_port} (advertise {pub_addr}, global {global_kbps} kbps)"
    );
    let relay = RelayState {
        registry: Mutex::new(Registry::new()),
        socket,
        secret: rand::random(),
        global_bucket: Mutex::new(TokenBucket::new(global_kbps)),
        quota: Mutex::new(QuotaLedger::new(crate::admin::current_period())),
        pub_addr,
    };
    state
        .relay
        .set(relay)
        .map_err(|_| anyhow::anyhow!("relay already spawned"))?;
    tokio::spawn(data_plane(state.clone()));
    tokio::spawn(patrol(state));
    Ok(())
}

/// 转发任务:select 收包处理与 5s 记账刷库 tick。
async fn data_plane(state: Arc<AppState>) {
    let relay = state
        .relay
        .get()
        .expect("data_plane 只由 spawn_relay 在装配后 spawn");
    let mut buf = vec![0u8; 65_535];
    let mut flush_tick = tokio::time::interval_at(
        tokio::time::Instant::now() + FLUSH_INTERVAL,
        FLUSH_INTERVAL,
    );
    loop {
        tokio::select! {
            r = relay.socket.recv_from(&mut buf) => match r {
                Ok((n, src)) => handle_datagram(&state, relay, &buf[..n], src).await,
                Err(e) => {
                    // 瞬态错误(网络栈抖动等)退避续跑,不崩任务
                    tracing::warn!("relay udp recv: {e}");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            },
            _ = flush_tick.tick() => flush_quietly(&state),
        }
    }
}

/// 单个数据报的钉死处理序列:tag 鉴权 → 槽位重绑 → 会话桶/全局桶闸门 →
/// 转发 → 记账 → 超配额杀;任何一步不过即静默丢弃,不 panic。
async fn handle_datagram(state: &AppState, relay: &RelayState, buf: &[u8], src: SocketAddr) {
    // 畸形:不足 16B 头
    if buf.len() < 16 {
        return;
    }
    // 1) 逐包鉴权:tag → (sid, slot);未登记标签(伪造/错票/过期)静默丢
    let Some((sid, slot)) = relay.registry.lock().unwrap().lookup(&buf[..16]) else {
        return;
    };
    let now = std::time::Instant::now();
    // 2) 会话定位 + 槽位重绑(裁定 D:换源地址无歧义,不做首包钉死)+ 会话桶
    let (peer_addr, account_id, admitted) = {
        let mut reg = relay.registry.lock().unwrap();
        let Some(s) = reg.get_mut(sid) else { return }; // 陈旧 tag 索引兜底
        s.slot_addrs[slot as usize] = Some(src);
        s.last_refresh = now;
        let bucket = match slot {
            0 => &mut s.bucket_a,
            _ => &mut s.bucket_b,
        };
        // rate_kbps = 0 为"不限"(admin 口径),跳过会话桶;全局限速独立生效
        let admitted = s.rate_kbps == 0 || bucket.try_take(buf.len(), now);
        (s.slot_addrs[(1 - slot) as usize], s.account_id.clone(), admitted)
    };
    if !admitted {
        return;
    }
    // 3) 全局桶:所有会话共享总闸,与每会话限速叠加;rate=0 为"不限"跳过。
    //    两桶先后 try_take:全局拒时会话桶已扣即损耗(丢弃式限速的固有损耗,
    //    偏保守方向,无害)。
    let global_admits = {
        let mut bucket = relay.global_bucket.lock().unwrap();
        bucket.rate_kbps() == 0 || bucket.try_take(buf.len(), now)
    };
    if !global_admits {
        return;
    }
    // 4) 对侧槽有地址则转发载荷(strip 16B 头),无则丢(对端未握手):
    //    未上线的报文随转发一并终止,不进入后续记账
    match peer_addr {
        Some(peer) => {
            if let Err(e) = relay.socket.send_to(&buf[16..], peer).await {
                tracing::debug!("relay forward to {peer}: {e}");
            }
        }
        None => return,
    }
    // 5) 会话字节累计(整报文含头;台账为权威,此处只作会话级视角)
    if let Some(s) = relay.registry.lock().unwrap().get_mut(sid) {
        s.bytes_accounted = s.bytes_accounted.saturating_add(buf.len() as u64);
    }
    // 6) 台账记账(整报文含头,双向同价)→ 超余量立即杀(裁定 C:内存检测)
    // charge 返回 true = 记账成功,false = 超出余量须杀
    let admitted_by_quota = {
        let mut ledger = relay.quota.lock().unwrap();
        if !ledger.known(&account_id) {
            let conn = state.db.lock().unwrap();
            if let Err(e) = ledger.seed(&conn, &account_id) {
                tracing::warn!("relay quota seed {account_id}: {e:#}");
            }
        }
        ledger.charge(&account_id, buf.len() as u64)
    };
    if !admitted_by_quota {
        // 立即摘除 + 双端通告;摘除失败(已被巡检收走)则不重复通告
        if relay.registry.lock().unwrap().remove(sid).is_some() {
            notify_relay_dead(state, &account_id, sid, "quota");
        }
    }
    // 7) 1 MiB 阈值即刷(5s 周期由数据面 tick 兜底)
    if relay.quota.lock().unwrap().dirty_total() >= FLUSH_BYTES_THRESHOLD {
        flush_quietly(state);
    }
}

/// 巡检任务:60s 周期调 [`sweep_refresh`](裁定 C 的"60s 内生效"载体)。
async fn patrol(state: Arc<AppState>) {
    let mut tick = tokio::time::interval_at(
        tokio::time::Instant::now() + PATROL_INTERVAL,
        PATROL_INTERVAL,
    );
    loop {
        tick.tick().await;
        if let Err(e) = sweep_refresh(&state) {
            tracing::warn!("relay patrol: {e:#}");
        }
    }
}

/// 巡检一次(可独立调用;后台巡检任务仅按周期调它,测试/运维可随时直调):
/// 空闲回收 + 逐会话重读 relay_config(disabled → 杀;rate 变更 → 热调)+
/// 台账月界滚动(旧期尾巴落库、新期余量重认)+ 兜底刷库。中继未启动时为 no-op。
pub fn sweep_refresh(state: &AppState) -> anyhow::Result<()> {
    let Some(relay) = state.relay.get() else { return Ok(()) };
    let now = std::time::Instant::now();

    // 1) 空闲回收:先快照拿 sid→account(通告要账号),sweep 后锁外逐一致通告
    let swept: Vec<(u64, String)> = {
        let mut reg = relay.registry.lock().unwrap();
        let snap: HashMap<u64, String> = reg
            .snapshot()
            .into_iter()
            .map(|(sid, acc, _)| (sid, acc))
            .collect();
        reg.sweep_idle(now)
            .into_iter()
            .filter_map(|sid| snap.get(&sid).map(|acc| (sid, acc.clone())))
            .collect()
    };
    for (sid, acc) in &swept {
        notify_relay_dead(state, acc, *sid, "idle");
    }

    // 2) 逐会话重读 relay_config:disabled → 杀;rate 变更 → 热调(裁定 C)
    let mut kills: Vec<(u64, String)> = Vec::new();
    let mut rate_sets: Vec<(u64, u32)> = Vec::new();
    {
        let conn = state.db.lock().unwrap();
        // 同账号多会话(≤2)共享一次配置读
        let mut cfg_cache: HashMap<String, (bool, u32)> = HashMap::new();
        for (sid, acc, cur_rate) in relay.registry.lock().unwrap().snapshot() {
            if swept.iter().any(|(s, _)| *s == sid) {
                continue; // 已被空闲回收,不重复处置
            }
            let cfg = match cfg_cache.get(&acc) {
                Some(c) => *c,
                None => {
                    // relay_config 行缺失的遗留账号按停用兜底(fail-closed)
                    let c = relay_config_of(&conn, &acc).unwrap_or((false, 0));
                    cfg_cache.insert(acc.clone(), c);
                    c
                }
            };
            if !cfg.0 {
                kills.push((sid, acc));
            } else if cfg.1 != cur_rate {
                rate_sets.push((sid, cfg.1));
            }
        }
    }
    {
        let mut reg = relay.registry.lock().unwrap();
        for (sid, _) in &kills {
            reg.remove(*sid);
        }
        for (sid, rate) in &rate_sets {
            if let Some(s) = reg.get_mut(*sid) {
                s.rate_kbps = *rate;
                s.bucket_a.set_rate(*rate, now);
                s.bucket_b.set_rate(*rate, now);
            }
        }
    }
    for (sid, acc) in &kills {
        notify_relay_dead(state, acc, *sid, "disabled");
    }

    // 3) 月界滚动:台账 period 落后于当前周期 → 旧期尾巴按旧 period 落库,
    //    再切到当期并按 entitlement 重认余量(新周期 usage 从零起算)
    let cur = crate::admin::current_period();
    let mut ledger = relay.quota.lock().unwrap();
    if ledger.period != cur {
        {
            let conn = state.db.lock().unwrap();
            ledger.flush(&conn)?;
        }
        ledger.period = cur;
        let conn = state.db.lock().unwrap();
        for acc in ledger.accounts.keys().cloned().collect::<Vec<_>>() {
            if let Some(q) = ledger.accounts.get_mut(&acc) {
                // 读取失败保守保留旧余量(下一轮巡检再试)
                q.remaining = crate::admin::relay_entitlement(&conn, &acc)
                    .map(|(_, _, rem)| rem)
                    .unwrap_or(q.remaining);
            }
        }
    }
    // 4) 巡检兜底刷库(数据面另有 1 MiB / 5s 自动阈值)
    let conn = state.db.lock().unwrap();
    ledger.flush(&conn)?;
    Ok(())
}

/// relay_config 行 → (enabled, rate_kbps)。
fn relay_config_of(conn: &Connection, account_id: &str) -> rusqlite::Result<(bool, u32)> {
    conn.query_row(
        "SELECT enabled != 0, rate_kbps FROM relay_config WHERE account_id=?1",
        [account_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
}

/// 会话终止通告:向该账号全部在线设备推 RelaySession{sid, alive:false, reason}。
/// 死因 `reason` 取 quota|disabled|idle,客户端据以区分配额杀/停用杀/空闲回收;
/// peer_device 留空(死帧无对端语义),端上按 sid 路由。注册表只存账号不存两端
/// 设备号,按账号广播即覆盖两端;端上不认识的 sid 忽略即可(旧客户端走未知帧
/// 忽略路径,不断连)。
fn notify_relay_dead(state: &AppState, account_id: &str, sid: u64, reason: &str) {
    let frame = mtdss_proto::ws::WsServerFrame {
        frame: Some(mtdss_proto::ws::ws_server_frame::Frame::RelaySession(
            mtdss_proto::ws::RelaySession {
                sid,
                alive: false,
                tag: Vec::new(),
                relay_addr: String::new(),
                expires_at: 0,
                peer_device: String::new(),
                reason: reason.into(),
                purpose: String::new(), // 死帧按 sid 路由,无平面语义
            },
        )),
    };
    let msg = axum::extract::ws::Message::Binary(frame.encode_to_vec().into());
    for (_, peer) in state.presence.account_peers(account_id) {
        let _ = peer.tx.try_send(msg.clone());
    }
}

/// 刷库(容忍失败:记账下轮阈值/巡检再试),无脏字节时不开 DB 锁。
fn flush_quietly(state: &AppState) {
    let Some(relay) = state.relay.get() else { return };
    let mut ledger = relay.quota.lock().unwrap();
    if ledger.dirty_total() == 0 {
        return;
    }
    let conn = state.db.lock().unwrap();
    if let Err(e) = ledger.flush(&conn) {
        tracing::warn!("relay quota flush: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_global_kbps_defaults_and_fallback() {
        assert_eq!(parse_global_kbps(None), 2500);
        assert_eq!(parse_global_kbps(Some("2500")), 2500);
        assert_eq!(parse_global_kbps(Some("0")), 0); // 0=不限
        assert_eq!(parse_global_kbps(Some(" 4096 ")), 4096); // 容忍空白
        assert_eq!(parse_global_kbps(Some("abc")), 2500); // 坏值回退默认
        assert_eq!(parse_global_kbps(Some("")), 2500);
    }

    #[test]
    fn quota_ledger_charge_and_month_boundary() {
        let mut ledger = QuotaLedger::new("2000-01".into());
        assert!(!ledger.known("acc"));
        // 未登记按超限(fail-closed)
        assert!(!ledger.charge("acc", 10));
        // seed 后按余量扣减
        ledger.accounts.insert(
            "acc".into(),
            AccountQuota { remaining: 100, dirty: 0 },
        );
        assert!(ledger.charge("acc", 40));
        assert_eq!(ledger.accounts["acc"].remaining, 60);
        assert!(!ledger.charge("acc", 61)); // 超限
        assert_eq!(ledger.accounts["acc"].remaining, 0);
        assert_eq!(ledger.dirty_total(), 40 + 61);
        // 不限余量只累脏字节
        ledger.accounts.insert(
            "inf".into(),
            AccountQuota { remaining: u64::MAX, dirty: 0 },
        );
        assert!(ledger.charge("inf", u64::MAX / 2));
        assert!(ledger.charge("inf", u64::MAX / 2));
        assert_eq!(ledger.accounts["inf"].remaining, u64::MAX);
    }
}
