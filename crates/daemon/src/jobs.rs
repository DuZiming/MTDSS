//! 传输作业注册表 + upload/download/local_copy/remote_copy 驱动 + 速度
//! EMA + 事件节流(M3c Task 6/7/8)。
//!
//! 生命周期不变量(取消/暂停的语义钉死,改动前必读):
//!
//! - **取消 = 置 watch + abort 驱动任务**。[`Jobs::cancel`] 在同一临界区里
//!   查终态 → 发取消 watch(流内 select 感知)→ 终态落定 `cancelled` →
//!   取走 JoinHandle 并 abort——连 run_send_with 的重试循环一并杀,**绝不
//!   复活**(JoinHandle 取走后注册表不再持有任务面)。abort 与流内 select
//!   是双保险:abort 在任务挂起点立即生效,select 让取消在退避睡眠/暂停
//!   门等待中也能以干净错误收束。终态粘滞:done/failed/cancelled 一旦
//!   落定,后续 cancel/finish/翻转全部无操作(cancel 与完成竞速时先到者
//!   胜,后到者只落空,不回滚)。
//!
//! - **暂停 = 双侧共享 PauseGate**(任务注册表持一份,流进
//!   SendOptions.pause)。门在发送引擎的每波前/每条块流开启前挂起(在途
//!   写不受影响,接收侧靠 QUIC 流控背压);重试循环轮间也持同一门——
//!   **暂停期间不烧重连预算、不退避、不重打洞**;若网络失败发生时门处于
//!   暂停态(连接死于用户挂起期间,如接收端 idle 超时),该失败按暂停
//!   归因,attempt 不增(预算 re-arm),门上等恢复后按原预算重试。
//!   pause + 断链 + resume 由此收敛:恢复后既有重试循环重新打洞,接收方
//!   按 .mtdss 记录对账断点续传。可见状态随之翻转(running↔paused,事件
//!   即发),但驱动起步时若用户已暂停(queued 期 pause),状态保持
//!   paused——门会在流内生效,不会出现「状态 running 实际挂死」的假象。
//!
//! - **事件契约**:`event.transfer {id,kind,state,files_done,files_total,
//!   bytes_done,bytes_total,speed_bps,error?}`——状态翻转即发;进度节流
//!   ≥400ms(首条进度即发,保证快传也有最早反馈;终态帧恒即发,不受节流
//!   约束)。transfer.wait 长轮询由 Notify 唤醒(enable-先于-查 的登记序
//!   防漏唤醒)。
//!
//! - **cut 收尾**:cut=true 的 upload 在驱动成功后本地 `fs::delete_paths
//!   (force=false)` 删源(回收优先,尽力而为:失败仅留痕,不翻终态);
//!   删源先于 done 落定——wait 返回 done 时删源已完成,磁盘事实与状态
//!   一致(cancel 若落在本窗口,终态由先到者定)。
//!
//! - **并发面**:作业不经遗留 send 的互斥(code 2 只属遗留 send RPC);
//!   多作业并发合法。V1 已知限制:同一接收端的入件仍按常驻接收轮串行。
//!
//! - **RoundHandle/bind_incoming**:resident 接收轮与作业的双向句柄
//!   (进度入注册表 + 作业侧取消/暂停直达该轮)。Task 7 接线:resident
//!   loop 在 ConnectRequest 处装配三件套并 `bind_incoming`(仅 download
//!   作业可作绑定目标——跨设备普通作业的 id 撞号不构成绑定,防误路由);
//!   cancel/pause 直达绑定的轮,进度回调随轮进 RecvCtl。终审修复补两
//!   面绑定纪律:绑定即把作业级暂停门现值**种子**进轮门(暂停可能落在
//!   轮装配前——orchestrate/打洞/重试间隙,作业级门无消费方,不抄即
//!   静默丢失);**终态作业拒绝绑定**并驱动该轮取消 watch(取消后仍在
//!   途的来件数据不得落地)。
//!
//! - **download(Task 7)**:操作端注册 download 作业 → ctrl `orchestrate`
//!   发给源设备(to_device=自己)→ 源端起 upload 驱动(回显同一 job_id,
//!   [`Jobs::accept_orchestration`],外部 id 已存在即拒)→ ConnectRequest
//!   带 job_id 回来 → resident loop 绑定轮路由进度/取消。轮成功即 done
//!   (resident loop 收口);轮失败作业保持 running——源端驱动自行重试,
//!   重来的 ConnectRequest 重绑续流,终局失败由 download 驱动周期
//!   `job_query` 探测源端作业态收口。取消 = 本地照常(cancel watch +
//!   abort + 轮取消)+ 独立哨任务在 cancelled 终态后 best-effort
//!   `cancel_job` 通知源端停重试(哨不经驱动任务,abort 杀不掉)。
//!   cut 收尾 = 本地 done 后对源端 ctrl `delete`(回收优先,尽力而为)。
//!   计数面:数据在远端,预扫不可为——files/bytes 总数恒 0(未知),
//!   RecvProgress 逐步填 done/bytes。轮失败作业保持 running——源端驱动
//!   自行重试,重来的 ConnectRequest 重绑续流,终局失败由 download 驱动
//!   周期 `job_query` 探测源端作业态收口;查询持续不可达超
//!   [`SOURCE_POLL_FAIL_BOUND`] 亦按失败收口(对端真下线不悬挂)。
//!
//! - **local_copy(Task 7;M4b 移交债①策略化)**:本机复制(spawn_blocking
//!   跑 `fs::copy_paths_with`,cut=false)或移动快路(`fs::move_entries_
//!   with` 逐顶层条目 rename,cut=true;文件覆盖语义、目录撞名逐对失败),
//!   冲突策略(skip/overwrite)与传输面同口径经 RPC 下发进驱动——同设备
//!   复制不再有「35s RPC 同步等待」的旧脸面,作业卡/取消/暂停全量可用。
//!   纯本地操作不要求登录;fs 层无逐字节进度,作业卡以预扫计数呈现,完成
//!   时一次性补齐 done/bytes;策略 skip 命中数落 `JobSnap.skipped`(各
//!   kind 缺省 0),事件与快照同带。
//!
//! - **remote_copy(Task 8)**:操作端 X 编排**两台远端设备间**的直传
//!   (from 源 → to 目标;数据不经 X 落地,X home 无传输数据)。预扫(from
//!   端逐源 stat × to 端目标目录 list;目录缺失 = 无冲突,列表截断按顶层
//!   可见集比对 + 警示;命中同名仅留痕,策略以调用方下发表为准)→
//!   orchestrate 发给 **from 设备**(其起回显 job_id 的 upload 驱动,
//!   ConnectRequest 定向 to)→ 本端卡片 1.5s 轮询 from 端 job_query 至终态
//!   并即时映射——done 无宽限窗(X 无本机数据面,from 发完 = to 已收口,
//!   与 download 的收端时差语义不同);failed/cancelled/missing 同 download
//!   口径,查询持续不可达超 [`SOURCE_POLL_FAIL_BOUND`](60s)亦按失败收口
//!   ——对端真下线不悬挂(单次瞬时失败不计,任一成功查询归零,取消不受
//!   此界约束)。cut 收尾 = done 后对 from 端 ctrl delete(回收优先,尽力
//!   而为);
//!   cancel = 本地终态 + 独立哨 cancel_job 发 from(哨不受驱动 abort 影响,
//!   同 download)。暂停面缺位(本机无数据流,门无消费方):set_paused 对
//!   remote_copy 直接拒,防「状态 paused 实际在传」假象。id 命名空间沿
//!   download 口径(`<本机 device_id>:<序号>`,回显键与 from 端自增面/其他
//!   操作端永不撞号)。计数面:数据在远端两端之间,预扫不可为——files/
//!   bytes 总数恒 0(未知)。已知 V1 限制:冲突策略顶层粒度,嵌套同名交
//!   接收端引擎安全网。
//!
//! - **匿名 incoming 卡(Task 7)**:ConnectRequest 不带 job_id(legacy
//!   send / 对端无任务面)→ resident loop 注册 kind="incoming" 的匿名卡
//!   并绑定轮——每笔传输都有可见卡片。轮失败即落 failed(源端重试会起新
//!   卡),轮成功落 done。

use crate::ctrl::CtrlErr;
use crate::state::DaemonState;
use mtdss_core::transport::intake::TransferPolicy;
use mtdss_core::transport::transfer::{PauseGate, RecvProgress, SendProgress};
use mtdss_ipc::EventFrame;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::{watch, Notify};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

/// 事件方法名:作业状态翻转(立即)与节流进度(≥400ms)。
pub(crate) const EV_TRANSFER: &str = "event.transfer";
/// 进度事件最小间隔(节流):进度帧两两间隔不得小于它;状态翻转帧不受限。
const PROGRESS_EVENT_MIN_GAP: Duration = Duration::from_millis(400);
/// 速度 EMA 采样窗口(毫秒)。
const SPEED_WINDOW_MS: u64 = 5_000;
/// 速度采样保留上限(窗口过滤之外的无界硬帽,防长跑作业样本无界增长)。
const SPEED_SAMPLE_CAP: usize = 256;
/// 策略上限(全局约束):skip+overwrite 合计条数。
pub(crate) const POLICY_MAX_ENTRIES: usize = 2048;
/// 策略上限(全局约束):单条字节数。
pub(crate) const POLICY_MAX_ENTRY_BYTES: usize = 4096;
/// 源端作业态查询持续失败的上界(download/remote_copy 两驱动共用,评审
/// Important:轮询缝查询失败原为无限重试——A 中途下线会让操作端卡片永远
/// running,只有用户取消能救)。连续查询不可达(打洞失败/对端掉线)超过
/// 它 → 本端卡片失败收口,文案可行动(可重试)。单次瞬时失败不烧计数:
/// 任一成功查询即归零;取消走 waiter 臂/任务 abort,即时胜出不受此界约束。
/// 60s ≈ 若干次打洞尝试的自然上界,把「偶发断链」与「真下线」分开。
const SOURCE_POLL_FAIL_BOUND: Duration = Duration::from_secs(60);
/// 终态保留上限(M4a-1):注册表至多保留这么多条终态作业,超出即在最旧
/// 一侧剪枝(活跃作业不计入、永不驱逐)。驱逐只发生在插入新作业时
/// ([`Jobs::insert_job`] 同临界区),终态翻转不触发。
const TERMINAL_RETAIN_CAP: usize = 100;

/// 是否终态(粘滞:落定后任何再翻转无操作)。
fn is_terminal_state(s: &str) -> bool {
    matches!(s, "done" | "failed" | "cancelled")
}

/// 毒锁恢复取锁(M4a-2):std Mutex 在持锁 panic 后永久 PoisonError,旧
/// `.lock().unwrap()` 会把一次 panic 放大成该锁所有后续使用方全部 panic
/// (守护进程连锁崩)。此处 Err(poisoned) → `into_inner()` 取回数据照常
/// 读写——毒化只说明持锁段被打断,数据结构本身未坏(本模块临界区均为
/// 毫秒级简单字段改写)。jobs.rs 定义,flow.rs(SendTracker)复用。
pub(crate) fn lock_ok<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// id 排序键(snapshot 与终态驱逐共用):裸数字 id 即数字本身;命名空间
/// id(`<dev>:<seq>`)取末段冒号后的序号——两型同一排序面。解析失败的
/// 畸形 id 排末尾(防御,不 panic)。
fn job_seq(id: &str) -> u64 {
    id.rsplit(':')
        .next()
        .unwrap_or(id)
        .parse::<u64>()
        .unwrap_or(u64::MAX)
}

/// 终态保留上限收尾(M4a-1):终态条数超 [`TERMINAL_RETAIN_CAP`] 时,按
/// id seq 升序驱逐最旧若干条(排序键复用 snapshot 的 [`job_seq`]);活跃
/// 作业不在候选集,永不驱逐。只在插入新作业的临界区调用(驱逐只发生在
/// 插入时,终态翻转不触发——满员后终态化的作业到下一次插入才被剪)。
fn retain_terminal_cap(map: &mut HashMap<String, JobHandle>) {
    if map.values().filter(|h| h.core.is_terminal()).count() <= TERMINAL_RETAIN_CAP {
        return;
    }
    let mut terminal: Vec<(u64, String)> = map
        .iter()
        .filter(|(_, h)| h.core.is_terminal())
        .map(|(id, _)| (job_seq(id), id.clone()))
        .collect();
    terminal.sort_unstable_by_key(|(n, _)| *n);
    let evict = terminal.len() - TERMINAL_RETAIN_CAP;
    for (_, id) in terminal.iter().take(evict) {
        map.remove(id);
    }
}

/// 策略上限校验(全局约束:合计 ≤2048 条、单条 ≤4096 字节);upload/
/// download 起作业与 ctrl copy/move RPC(api.rs)共用同一校验面。
pub(crate) fn check_policy_limits(policy: &TransferPolicy) -> Result<(), String> {
    if policy.skip.len() + policy.overwrite.len() > POLICY_MAX_ENTRIES {
        return Err(format!(
            "策略超限:skip+overwrite 合计 {} 条 > 上限 {POLICY_MAX_ENTRIES}",
            policy.skip.len() + policy.overwrite.len()
        ));
    }
    for e in policy.skip.iter().chain(policy.overwrite.iter()) {
        if e.len() > POLICY_MAX_ENTRY_BYTES {
            return Err(format!(
                "策略条目超长: {} 字节 > 上限 {POLICY_MAX_ENTRY_BYTES}",
                e.len()
            ));
        }
    }
    Ok(())
}

// ---------- 注册表 ----------

/// 非终态作业计数面(Task 13,托盘 tooltip 数据源):原子计数 + watch 广播。
/// 建队 +1(JobCore::new)、终态落定 -1(transition 终态分支;终态粘滞保证
/// 不重复扣)。watch 供托盘线程同步轮询(与 login watch 同一样板)。
struct CountWatch {
    tx: tokio::sync::watch::Sender<usize>,
    n: std::sync::atomic::AtomicUsize,
}

impl CountWatch {
    fn new() -> Self {
        Self {
            tx: watch::channel(0).0,
            n: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// 增减计数并广播(+1 建队 / -1 终态;无接收者时静默丢弃)。增减与
    /// 建队/终态严格配对,不会下溢。
    fn bump(&self, d: isize) {
        let n = if d >= 0 {
            self.n.fetch_add(d as usize, Ordering::Relaxed) + d as usize
        } else {
            self.n.fetch_sub((-d) as usize, Ordering::Relaxed) - (-d) as usize
        };
        let _ = self.tx.send(n);
    }
}

/// 传输作业注册表:id 单调分配(u64 计数,字符串对外),map 持全部作业
/// (终态保留上限 100,驱逐只发生在插入新作业时——transfer.list 的历史
/// 面与 wait 的存在性都依赖它)。
/// 键空间(Task 7 修复):upload/local_copy/incoming 用裸数字自增 id;
/// **download 用 `<本机 device_id>:<序号>` 命名空间**——下载 id 会经
/// orchestrate 回显进源端注册表并复用为 bind 键,嵌操作端 id 后与源端
/// 自增面/其他操作端永不撞号(评审 Critical#1)。
pub(crate) struct Jobs {
    next_id: AtomicU64,
    map: Mutex<HashMap<String, JobHandle>>,
    /// 非终态作业计数 watch(Task 13):托盘 tooltip「N 个传输中」。
    count: Arc<CountWatch>,
}

/// 注册表条目:核心状态(快照/事件/唤醒)+ 控制面(暂停门/取消 watch/
/// 驱动任务)+ 接收轮槽位(Task 7)。
struct JobHandle {
    core: Arc<JobCore>,
    pause: Arc<PauseGate>,
    cancel_tx: watch::Sender<bool>,
    /// 驱动任务句柄:cancel 取走并 abort(取走即注册表不再持任务面,
    /// 不复活);自然结束后留在槽内(abort 对已完成任务是 no-op)。
    task: Mutex<Option<JoinHandle<()>>>,
    /// resident 接收轮双向句柄槽位(Task 7 接线;cancel/pause 已直达)。
    round: Mutex<Option<RoundHandle>>,
}

/// resident 接收轮与作业的双向句柄(Task 7 download 侧接线;签名钉死)。
pub(crate) struct RoundHandle {
    /// 该轮的取消开关(resident loop 在轮内 select 它)。
    pub cancel: watch::Sender<bool>,
    /// 该轮的暂停门(接收主循环挂起,发送端被 QUIC 流控背压)。
    pub pause: Arc<PauseGate>,
    /// 进度回调(RecvProgress → 注册表):resident loop 装配进 RecvCtl,
    /// 注册表经 [`Jobs::track_recv`] 聚合。
    pub progress: Arc<dyn Fn(&RecvProgress<'_>) + Send + Sync>,
}

/// `bind_incoming` 结果(终审修复):三分支让 resident loop 区分「未命中
/// 降级匿名卡」与「终态拒绝弃轮」——旧 bool 面把两者压成同一值,取消后
/// 在途的来件会被降级匿名卡照常收下(已取消下载的数据落地)。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BindOutcome {
    /// 占槽成功;作业级暂停门现值已抄进轮门(轮装配前的暂停不丢)。
    Bound,
    /// 作业存在、种类相符但已终态:该轮取消 watch 已被驱动,调用方弃轮。
    RefusedTerminal,
    /// 无此作业或种类不符(降级匿名卡)。
    Miss,
}

/// 作业快照(transfer.list 的数据源;字段钉死,Task 7/8/11/12 消费)。
pub(crate) struct JobSnap {
    pub id: String,
    pub kind: &'static str,
    pub state: String,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub speed_bps: u64,
    /// 策略 skip 命中数(M4b 移交债①,local_copy 驱动回填;各 kind 缺省
    /// 0——传输面 skip 文件按 FileDone 计入 done,不重复计数)。
    pub skipped: u64,
    pub src: String,
    pub dst: String,
    pub error: Option<String>,
}

/// 驱动起步前的作业静态面(构造期一次定形:预扫计数与展示字段)。
struct JobMeta {
    files_total: u64,
    bytes_total: u64,
    /// rel → 字节(FileDone 无总量信息时补齐 done=total,让 skip 文件也
    /// 计满字节;来源 = 预扫 local_tree_rels)。
    rel_sizes: HashMap<String, u64>,
    src: String,
    dst: String,
}

/// 单作业可变状态(锁内持改;进度回调/控制面/驱动收尾三方共用此锁,
/// 所有操作毫秒级,无长阻塞)。
struct JobMut {
    state: String,
    /// 已完整落盘的 rel(发送侧 FileDone;skip 文件接收端也回 FileDone)。
    done: HashSet<String>,
    /// rel → (done, total):done 跨轮取最大值(与 SendTracker 同口径)。
    file_bytes: HashMap<String, (u64, u64)>,
    /// 速度采样 (ms since started, 累计字节)。
    samples: Vec<(u64, u64)>,
    /// 策略 skip 命中数(M4b 移交债①;local_copy 驱动终局前一次性回填)。
    skipped: u64,
    started: Instant,
    last_progress_emit: Option<Instant>,
    error: Option<String>,
}

/// 单作业核心:静态标识 + 可变状态 + 终态唤醒 + 事件出口。
struct JobCore {
    id: String,
    kind: &'static str,
    meta: JobMeta,
    m: Mutex<JobMut>,
    /// 终态长轮询唤醒(transfer.wait);翻转通知用 notify_waiters。
    notify: Notify,
    /// 事件广播出口(与 daemon 全事件面同一 channel)。
    events: tokio::sync::broadcast::Sender<EventFrame>,
    /// 非终态计数面(Task 13):建队 +1(new)/终态 -1(transition),与
    /// 注册表共享同一 CountWatch。
    count: Arc<CountWatch>,
}

impl JobCore {
    /// 建队(queued):计数 +1(托盘「N 个传输中」面);不在此发事件——
    /// 调用方在建队原子段内发首帧。
    fn new(
        id: String,
        kind: &'static str,
        meta: JobMeta,
        events: tokio::sync::broadcast::Sender<EventFrame>,
        count: Arc<CountWatch>,
    ) -> Self {
        count.bump(1);
        Self {
            id,
            kind,
            meta,
            m: Mutex::new(JobMut {
                state: "queued".into(),
                done: HashSet::new(),
                file_bytes: HashMap::new(),
                samples: Vec::new(),
                skipped: 0,
                started: Instant::now(),
                last_progress_emit: None,
                error: None,
            }),
            notify: Notify::new(),
            events,
            count,
        }
    }

    /// 锁内快照投影。
    fn snap_locked(&self, m: &JobMut) -> JobSnap {
        let bytes_done: u64 = m.file_bytes.values().map(|(d, _)| *d).sum();
        JobSnap {
            id: self.id.clone(),
            kind: self.kind,
            state: m.state.clone(),
            files_done: m.done.len() as u64,
            files_total: self.meta.files_total,
            bytes_done,
            bytes_total: self.meta.bytes_total,
            speed_bps: crate::ops::ema_speed(&m.samples, SPEED_WINDOW_MS),
            skipped: m.skipped,
            src: self.meta.src.clone(),
            dst: self.meta.dst.clone(),
            error: m.error.clone(),
        }
    }

    /// 事件帧(进度与翻转共用同一字段面;error 为 null 或字符串)。
    fn emit_locked(&self, m: &JobMut) {
        let snap = self.snap_locked(m);
        let _ = self.events.send(EventFrame {
            method: EV_TRANSFER.into(),
            params: json!({
                "id": snap.id,
                "kind": snap.kind,
                "state": snap.state,
                "files_done": snap.files_done,
                "files_total": snap.files_total,
                "bytes_done": snap.bytes_done,
                "bytes_total": snap.bytes_total,
                "speed_bps": snap.speed_bps,
                "skipped": snap.skipped,
                "error": snap.error,
            }),
        });
    }

    /// 状态翻转。终态粘滞(已终态 → 无操作);非终态同态翻转幂等无事件。
    /// 终态落定:置状态 + 事件即发 + 计数 -1(托盘「N 个传输中」面)+
    /// notify_waiters(唤醒全部 wait 长轮询)。
    fn transition(&self, new_state: &str, error: Option<String>) {
        let mut m = lock_ok(&self.m);
        if is_terminal_state(&m.state) {
            return; // 终态粘滞:cancel/完成竞速,先到者胜
        }
        if is_terminal_state(new_state) {
            m.state = new_state.to_string();
            m.error = error;
            self.emit_locked(&m);
            drop(m);
            self.count.bump(-1); // 终态粘滞保证只扣一次
            self.notify.notify_waiters();
            return;
        }
        if m.state == new_state {
            return;
        }
        m.state = new_state.to_string();
        self.emit_locked(&m);
    }

    /// 驱动起步翻转 running;用户在 queued 期已暂停 → 保持 paused(门在
    /// 流内生效,不制造「状态 running 实际挂死」的假象;resume 后由
    /// transition("running") 对齐)。
    fn begin_running(&self, gate_paused: bool) {
        if gate_paused {
            let m = lock_ok(&self.m);
            if m.state == "paused" {
                return; // 保持暂停态(非终态,无需事件)
            }
        }
        self.transition("running", None);
    }

    /// 暂停/恢复的可见状态翻转(门已由调用方先行翻转)。
    fn transition_paused(&self, paused: bool) {
        self.transition(if paused { "paused" } else { "running" }, None);
    }

    /// 终态落定(done/failed;cancel 走注册表的 mark_cancelled 同一通道)。
    fn finish(&self, state: &str, error: Option<String>) {
        self.transition(state, error);
    }

    /// 发送进度回调(SendProgress → 聚合计数 + 速度采样 + 节流事件)。
    /// 在发送引擎上下文同步调用:只做毫秒级锁内聚合与非阻塞广播。
    fn track_send(&self, p: &SendProgress<'_>) {
        match p {
            SendProgress::FileDone { rel } => self.track_common(Some(rel), None),
            SendProgress::Bytes { rel, done, total } => {
                self.track_common(None, Some((rel, *done, *total)))
            }
        }
    }

    /// 接收进度回调(RecvProgress → 同一聚合通道,Task 7 download/匿名
    /// 来件面):与 SendProgress 同形(FileDone{rel}/Bytes{rel,done,total}),
    /// 注册表侧单一聚合实现,两侧驱动各一行适配——不重复记账数学。
    fn track_recv(&self, p: &RecvProgress<'_>) {
        match p {
            RecvProgress::FileDone { rel } => self.track_common(Some(rel), None),
            RecvProgress::Bytes { rel, done, total } => {
                self.track_common(None, Some((rel, *done, *total)))
            }
        }
    }

    /// 进度聚合共用体:done 集合 + per-rel 字节(done 跨轮取最大,与
    /// SendTracker 同口径)+ 速度采样(单调时刻 + 累计字节,窗口/容量双重
    /// 裁剪)+ 节流事件(首条即发,其后 ≥400ms;终态帧不在此路径)。
    fn track_common(&self, file_done: Option<&str>, bytes: Option<(&str, u64, u64)>) {
        let mut m = lock_ok(&self.m);
        if is_terminal_state(&m.state) {
            return; // 驱动收尾/取消后迟到的回调:丢弃
        }
        match (file_done, bytes) {
            (Some(rel), _) => {
                m.done.insert(rel.to_string());
                // FileDone 不带总量:用预扫 size 补齐(skip 文件无 Bytes 回调,
                // 否则 bytes_done 永远缺它一份)
                let total = m
                    .file_bytes
                    .get(rel)
                    .map(|(_, t)| *t)
                    .or_else(|| self.meta.rel_sizes.get(rel).copied())
                    .unwrap_or(0);
                m.file_bytes.insert(rel.to_string(), (total, total));
            }
            (None, Some((rel, done, total))) => {
                let e = m.file_bytes.entry(rel.to_string()).or_insert((0, total));
                if done > e.0 {
                    e.0 = done;
                }
                e.1 = total;
            }
            (None, None) => return,
        }
        // 速度采样:单调时刻 + 累计字节;窗口/容量双重裁剪
        let ms = m.started.elapsed().as_millis() as u64;
        let bytes_now: u64 = m.file_bytes.values().map(|(d, _)| *d).sum();
        m.samples.push((ms, bytes_now));
        let last_ms = m.samples[m.samples.len() - 1].0;
        m.samples.retain(|(t, _)| last_ms.saturating_sub(*t) <= SPEED_WINDOW_MS);
        if m.samples.len() > SPEED_SAMPLE_CAP {
            let drop_n = m.samples.len() - SPEED_SAMPLE_CAP;
            m.samples.drain(..drop_n);
        }
        // 节流:首条即发(快传也有最早反馈),其后 ≥400ms;终态帧不在此路径
        let due = m
            .last_progress_emit
            .map(|t| t.elapsed() >= PROGRESS_EVENT_MIN_GAP)
            .unwrap_or(true);
        if due {
            m.last_progress_emit = Some(Instant::now());
            self.emit_locked(&m);
        }
    }

    /// 本地复制完成面:全部文件 rel 一次性记满 done/bytes(复用 track 通道
    /// 的计数口径——fs 层无逐字节进度,作业卡以预扫计数呈现终值)。不在此
    /// 发事件:调用方随后 finish,终态帧自带终值。
    fn mark_all_done(&self) {
        let mut m = lock_ok(&self.m);
        if is_terminal_state(&m.state) {
            return;
        }
        for (rel, size) in &self.meta.rel_sizes {
            m.done.insert(rel.clone());
            m.file_bytes.insert(rel.clone(), (*size, *size));
        }
    }

    /// 策略 skip 命中数回填(M4b 移交债①):local_copy 驱动在终局前一次
    /// 性写入,终态帧(与 transfer.list 快照)自带终值。已终态(取消竞速
    /// 先到)不回写——粘滞语义下迟到账目无消费者。
    fn record_skipped(&self, n: u64) {
        let mut m = lock_ok(&self.m);
        if is_terminal_state(&m.state) {
            return;
        }
        m.skipped = n;
    }

    /// 当前快照。
    fn snap(&self) -> JobSnap {
        let m = lock_ok(&self.m);
        self.snap_locked(&m)
    }

    fn is_terminal(&self) -> bool {
        is_terminal_state(&lock_ok(&self.m).state)
    }
}

impl Jobs {
    pub(crate) fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            map: Mutex::new(HashMap::new()),
            count: Arc::new(CountWatch::new()),
        }
    }

    /// 非终态作业计数 watch 接收端(Task 13):托盘线程同步轮询「N 个
    /// 传输中」,与 login watch 同一样板。消费者(托盘)与单测各自存活:
    /// 托盘仅 Windows,单测双平台——any(windows, test) 门住,Linux 正式
    /// 构建不编译(否则 dead_code)。
    #[cfg(any(windows, test))]
    pub(crate) fn count_rx(&self) -> watch::Receiver<usize> {
        self.count.tx.subscribe()
    }

    /// 非终态作业数(M4b Task 10):原子读 CountWatch 计数。paused 计入
    /// ——暂停不是终态(D12:无独立 stalled 态,stalled 即 running);
    /// 更新器空闲门(Task 11)据此在有传输(含已暂停)时拒绝 apply。
    // Task 11(update.rs 空闲门)跨平台启用;当前仅单测消费。
    #[allow(dead_code)]
    pub(crate) fn active_count(&self) -> usize {
        self.count.n.load(Ordering::Relaxed)
    }

    /// 注册表唯一插入路径(M4a-1):插入 + 终态保留上限收尾(同一临界区
    /// ——驱逐只在此发生,见 [`retain_terminal_cap`])。upload/download/
    /// remote_copy/local_copy/incoming 各建队点与测试装配共用。
    fn insert_job(&self, id: String, h: JobHandle) {
        let mut map = lock_ok(&self.map);
        map.insert(id, h);
        retain_terminal_cap(&mut map);
    }

    /// 起一个 upload 作业(自动 id;download 编排的回显入口走
    /// [`Jobs::accept_orchestration`] → [`Jobs::start_upload_internal`])。
    ///
    /// 校验(超限/非法即 Err → RPC code 1):files 非空;dir 合法(Task 1
    /// 绝对形态口径,不接受空 = 本 RPC 显式携带定向目录,与遗留 send 的
    /// 可缺省语义分开);策略上限(全局约束:合计 ≤2048 条、单条 ≤4096
    /// 字节)。预扫为空(输入缺失/全符号链接)同样 Err——作业化传输有
    /// 同步反馈面,不把注定失败的编排放进异步驱动。
    pub(crate) async fn start_upload(
        &self,
        state: &Arc<DaemonState>,
        files: Vec<PathBuf>,
        to: &str,
        dir: &str,
        policy: TransferPolicy,
        cut_src: bool,
    ) -> Result<String, String> {
        self.start_upload_internal(state, None, files, to, dir, policy, cut_src)
            .await
    }

    /// start_upload 的内部本体:`explicit_id` 为 Some 时采用外部作业 id
    /// (download 编排的回显,Task 7),已存在即 Err(操作端重试换新 id,
    /// 不撞);None 走单调计数分配。
    #[allow(clippy::too_many_arguments)] // 内部本体吃下 start_upload 与编排回显两侧的参数面(回显侧多 explicit_id 一位)
    async fn start_upload_internal(
        &self,
        state: &Arc<DaemonState>,
        explicit_id: Option<String>,
        files: Vec<PathBuf>,
        to: &str,
        dir: &str,
        policy: TransferPolicy,
        cut_src: bool,
    ) -> Result<String, String> {
        if files.is_empty() {
            return Err("files 不能为空".into());
        }
        // dir 属于**接收设备**(upload 目标/编排回显 target_dir),平台未知
        // ——平台无关绝对形态预检(公网跨平台:Windows↔Linux 互通),真
        // 合法性由接收端落地时按其自身平台判定(非法 warn+回退默认目录)。
        if !crate::flow::remote_absolute_form(dir) {
            return Err(format!("dir 非法(须为绝对路径,Windows 或 Unix 形态): {dir}"));
        }
        check_policy_limits(&policy)?;
        if state.session().is_none() {
            return Err("未登录,请先 login.verify".into());
        }
        // 预扫:全树 rel(只读元数据)——files/bytes 总数与 FileDone 补齐表
        // (整树元数据遍历是阻塞调用,落线程池,直跑占死 async worker;
        // 与 ctrl 面同一收口纪律)
        let prescan_files = files.clone();
        let rels = tokio::task::spawn_blocking(move || crate::ops::local_tree_rels(&prescan_files))
            .await
            .map_err(|e| format!("扫描任务失败: {e}"))?;
        let mut rel_sizes = HashMap::new();
        let mut files_total = 0u64;
        let mut bytes_total = 0u64;
        for (rel, is_dir, size) in &rels {
            if !is_dir {
                files_total += 1;
                bytes_total += *size;
                rel_sizes.insert(rel.clone(), *size);
            }
        }
        if files_total == 0 {
            return Err("无可传内容(输入缺失、空目录或全为符号链接)".into());
        }
        let src = files
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(", ");
        let meta = JobMeta {
            files_total,
            bytes_total,
            rel_sizes,
            src,
            dst: format!("{to}:{dir}"),
        };

        let id = match explicit_id {
            Some(id) => {
                if lock_ok(&self.map).contains_key(&id) {
                    return Err(format!("任务 id 已存在,拒绝编排回显: {id}"));
                }
                // 计数器前推(评审 Critical#1a):外部 id 若为纯数字且 ≥ 自增
                // 面计数器,立即把计数器推到它之上——否则后续自增 mint 会
                // 造出同号作业,注册表插入静默覆盖回显条目(驱动 JoinHandle
                // 丢失、作业孤儿化)。命名空间 id(现役操作端形态)不含纯数
                // 字,天然不撞;此守卫兜住旧版/异常的纯数字回显。
                if let Ok(n) = id.parse::<u64>() {
                    self.next_id.fetch_max(n.saturating_add(1), Ordering::Relaxed);
                }
                id
            }
            None => self.next_id.fetch_add(1, Ordering::Relaxed).to_string(),
        };
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let pause = PauseGate::new();
        let core = Arc::new(JobCore::new(
            id.clone(),
            "upload",
            meta,
            state.events(),
            self.count.clone(),
        ));
        // 建队原子段:首帧事件(queued)在插入注册表前发出——job_id 尚未
        // 返回给调用方,不存在「外部已见事件却查无此业」的窗口。
        {
            let m = lock_ok(&core.m);
            core.emit_locked(&m);
        }
        // spawn 至插入之间无 await 点:spawn 不会立即执行,插入先于驱动
        // 首步,transfer.cancel 不存在找不到条目的竞窗。驱动包监督网(supervise_
        // driver):panic 意外退出 → failed 收口,JoinHandle 语义不变。块内
        // shadow:借用形参(&state/&to/&dir)先定形为 owned,core/pause 另
        // 克隆给驱动任务,块外原绑定留给 JobHandle。
        let driver = {
            let state = state.clone();
            let core = core.clone();
            let pause = pause.clone();
            let to = to.to_string();
            let dir = dir.to_string();
            tokio::spawn(supervise_driver(
                core.clone(),
                async move {
                    upload_driver(
                        state,
                        core,
                        UploadJob {
                            files,
                            to,
                            dir,
                            policy,
                            cut_src,
                        },
                        pause,
                        cancel_rx,
                    )
                    .await
                },
            ))
        };
        self.insert_job(
            id.clone(),
            JobHandle {
                core,
                pause,
                cancel_tx,
                task: Mutex::new(Some(driver)),
                round: Mutex::new(None),
            },
        );
        Ok(id)
    }

    /// 接受跨设备编排(ctrl `orchestrate` 响应分支,Task 7):在**源端**
    /// 起一个 upload 作业,回显操作端预先注册的 job_id(ConnectRequest 随
    /// 之携带,操作端 resident loop 据 id 绑定路由)。现役操作端 id 带
    /// 操作端命名空间(`opDev:seq`,见 `start_download`),与源端自身自增
    /// 键空间天然隔离;旧版纯数字 id 仍按原样回显(存在即拒 + 计数器前
    /// 推兜底,见 `start_upload_internal`)。校验:job_id 非空(空 id 无法
    /// 回绑)、to_device 非空、id 未被占用;其余校验(路径存在性/目录形
    /// 态/策略上限/登录)沿用 start_upload 的同步反馈面,失败文案经
    /// OrchestrateResponse.detail 回给操作端落 failed。
    pub(crate) async fn accept_orchestration(
        &self,
        state: &Arc<DaemonState>,
        job_id: &str,
        to_device: &str,
        paths: &[String],
        target_dir: &str,
        policy: TransferPolicy,
    ) -> Result<(), String> {
        if job_id.is_empty() {
            return Err("orchestrate 缺 job_id(无法回绑)".into());
        }
        if to_device.is_empty() {
            return Err("orchestrate 缺 to_device".into());
        }
        let files: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        // cut 恒 false:download 的剪切收尾在操作端(成功后对源端 ctrl
        // delete),源端 upload 驱动不动源
        self.start_upload_internal(
            state,
            Some(job_id.to_string()),
            files,
            to_device,
            target_dir,
            policy,
            false,
        )
        .await?;
        Ok(())
    }

    /// 起一个 download 作业(Task 7):注册 queued 卡 → spawn 驱动(orchestrate
    /// → 轮进度经绑定路由 → job_query 周期探测源端终局)→ spawn 取消通知哨
    /// (cancelled 终态后 best-effort 通知源端停驱动重试;独立任务不受驱动
    /// abort 影响)。
    ///
    /// 校验:device 不得是本机(本机复制请用 local_copy,api 层已拒,此处
    /// 兜底);paths 非空;dir 合法(本机落盘目录,Task 1 绝对形态口径);
    /// 策略上限;须已登录(orchestrate 经 ctrl 打洞需要会话)。计数面:数
    /// 据在远端,预扫不可为——files/bytes 总数恒 0(未知),RecvProgress
    /// 逐步填 done/bytes。
    pub(crate) fn start_download(
        &self,
        state: &Arc<DaemonState>,
        device: &str,
        paths: Vec<String>,
        dir: &str,
        policy: TransferPolicy,
        cut: bool,
    ) -> Result<String, String> {
        if state.self_device_id().as_deref() == Some(device) {
            return Err("下载源设备不能是本机(本机复制请用 local_copy)".into());
        }
        if paths.is_empty() {
            return Err("paths 不能为空".into());
        }
        if !crate::flow::absolute_form(dir) {
            return Err(format!("dir 非法(须为绝对路径,Task 1 口径): {dir}"));
        }
        check_policy_limits(&policy)?;
        if state.session().is_none() {
            return Err("未登录,请先 login.verify".into());
        }
        let src = format!("{device}:{}", paths.join(", "));
        let meta = JobMeta {
            files_total: 0,
            bytes_total: 0,
            rel_sizes: HashMap::new(),
            src,
            dst: dir.to_string(),
        };

        // id 命名空间(Task 7 修复:评审 Critical#1):`<本机 device_id>:<序号>`。
        // 下载 id 会经 orchestrate 回显进**源端**注册表,并与操作端 resident
        // loop 的 bind 键复用——裸数字 id 与源端自身作业计数同键空间:
        // ① 源端既有任意作业(一笔 legacy send 即可)即撞号回绝编排;
        // ② 两台操作端并发同名序号互相污染。嵌入操作端 id 后,回显键只可能
        // 命中「本机接受过的该操作端编排」,自增面(纯数字)永不与之相撞。
        let id = match state.self_device_id() {
            Some(dev) => format!("{}:{}", dev, self.next_id.fetch_add(1, Ordering::Relaxed)),
            None => return Err("未登录,请先 login.verify".into()),
        };
        // 取消面:驱动内不 select 取消 watch(registry.cancel 的 abort 直接
        // 杀任务,orchestrate/轮询的每个 await 点即时生效),rx 仅留通道
        // 完整性;真正的远端止步由取消通知哨承担。
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        let pause = PauseGate::new();
        let core = Arc::new(JobCore::new(
            id.clone(),
            "download",
            meta,
            state.events(),
            self.count.clone(),
        ));
        {
            let m = lock_ok(&core.m);
            core.emit_locked(&m);
        }
        // 块内 shadow 同 upload:借用形参定形 owned,块外原绑定留给
        // JobHandle 与取消哨(watchman 还要用 device)。
        let driver = {
            let state = state.clone();
            let core = core.clone();
            let pause = pause.clone();
            let src_device = device.to_string();
            let dir = dir.to_string();
            tokio::spawn(supervise_driver(
                core.clone(),
                async move {
                    download_driver(
                        state,
                        core,
                        DownloadJob {
                            src_device,
                            paths,
                            dir,
                            policy,
                            cut,
                        },
                        pause,
                    )
                    .await
                },
            ))
        };
        // 取消通知哨:独立任务等本地终态,cancelled 即通知源端停驱动重试
        // (abort 杀不到;done/failed 自然退出)。哨无 JoinHandle 追踪——
        // 作业终态即自灭,生命周期有界。
        let watchman_state = state.clone();
        let watchman_id = id.clone();
        let watchman_device = device.to_string();
        tokio::spawn(async move {
            // Err = 无此作业/已驱逐(spawn 与注册表插入竞窗的早到情形——
            // 旧失败快照口径同样即刻返回,通知面无变化)
            let Ok(fin) = watchman_state.jobs().wait(&watchman_id).await else {
                return;
            };
            if fin.state != "cancelled" {
                return;
            }
            // best-effort:连接失败静默——源端驱动反正因收端断链失败,重试
            // 预算耗尽自然终态;cancel_job 只是让重试更早止步
            if let Err(e) = watchman_state
                .ctrl()
                .cancel_job(&watchman_state, &watchman_device, &watchman_id)
                .await
            {
                tracing::debug!(job = %watchman_id, detail = %e.rpc().1, "通知源端取消失败(忽略)")
            }
        });
        self.insert_job(
            id.clone(),
            JobHandle {
                core,
                pause,
                cancel_tx,
                task: Mutex::new(Some(driver)),
                round: Mutex::new(None),
            },
        );
        Ok(id)
    }

    /// 起一个 remote_copy 作业(Task 8 A→B 编排):操作端 X 让 **from 设备**
    /// 把 paths 直传给 to 设备的 dir(数据不经 X 落地)。预扫(from 端逐源
    /// stat × to 端目标目录 list)在驱动内;编排/cancel_job/job_query/cut
    /// 收尾的 ctrl 对端都是 from。终局:本端卡片 1.5s 轮询 from 端
    /// job_query 即时映射(done 无宽限窗,见模块文档);done 且 cut → 对
    /// from 端 ctrl delete 源路径(回收优先,尽力而为)。
    ///
    /// 校验:from/to 都不得是本机(from=本机属 upload 面、to=本机属
    /// download 面,各有作业型,此处显式指路);from == to 拒(同机复制走
    /// ctrl copy 面,编排自发自收只会困死在自拨守卫上);paths 非空;dir
    /// 合法(to 设备视角绝对形态);策略上限;须已登录(id 命名空间要用)。
    /// 计数面:数据在远端两端之间,预扫不可为——files/bytes 总数恒 0(未
    /// 知)。id 命名空间沿 Task 7 修复口径(`<本机 device_id>:<序号>`,评审
    /// Critical#1):回显进 from 端注册表后,与 from 端自增面/其他操作端
    /// 永不撞号。
    #[allow(clippy::too_many_arguments)] // 编排面参数齐平 IPC 契约形态(from/paths/to/dir/policy/cut)
    pub(crate) fn start_remote_copy(
        &self,
        state: &Arc<DaemonState>,
        from: &str,
        paths: Vec<String>,
        to: &str,
        dir: &str,
        policy: TransferPolicy,
        cut: bool,
    ) -> Result<String, String> {
        let me = state
            .self_device_id()
            .ok_or_else(|| "未登录,请先 login.verify".to_string())?;
        if from == me {
            return Err("源设备不能是本机(本机→设备请用 upload)".into());
        }
        if to == me {
            return Err("目标设备不能是本机(设备→本机请用 download)".into());
        }
        if from == to {
            return Err("源与目标不得是同一台设备(同机复制请用 copy 面)".into());
        }
        if paths.is_empty() {
            return Err("paths 不能为空".into());
        }
        // dir 属于目标设备(to),平台未知——平台无关绝对形态预检(见
        // start_upload_internal 同注)。
        if !crate::flow::remote_absolute_form(dir) {
            return Err(format!("dir 非法(须为绝对路径,Windows 或 Unix 形态): {dir}"));
        }
        check_policy_limits(&policy)?;
        let src = format!("{from}:{}", paths.join(", "));
        let meta = JobMeta {
            files_total: 0,
            bytes_total: 0,
            rel_sizes: HashMap::new(),
            src,
            dst: format!("{to}:{dir}"),
        };

        let id = format!("{}:{}", me, self.next_id.fetch_add(1, Ordering::Relaxed));
        // 取消面:驱动内不 select 取消 watch(registry.cancel 的 abort 直接
        // 杀任务,预扫/orchestrate/轮询的每个 await 点即时生效);远端止步
        // 由取消通知哨承担。
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        let pause = PauseGate::new();
        let core = Arc::new(JobCore::new(
            id.clone(),
            "remote_copy",
            meta,
            state.events(),
            self.count.clone(),
        ));
        {
            let m = lock_ok(&core.m);
            core.emit_locked(&m);
        }
        // 块内 shadow 同 upload(块外 from 还要给取消哨)。
        let driver = {
            let state = state.clone();
            let core = core.clone();
            let pause = pause.clone();
            let from_dev = from.to_string();
            let to_dev = to.to_string();
            let dir = dir.to_string();
            tokio::spawn(supervise_driver(
                core.clone(),
                async move {
                    remote_copy_driver(
                        state,
                        core,
                        RemoteCopyJob {
                            from: from_dev,
                            paths,
                            to: to_dev,
                            dir,
                            policy,
                            cut,
                        },
                        pause,
                    )
                    .await
                },
            ))
        };
        // 取消通知哨(同 download):本地 cancelled 终态后 best-effort 通知
        // from 端停驱动重试(abort 杀不到;done/failed 自然退出,生命周期
        // 有界)。连接失败静默——from 端驱动因操作端不再轮询也不影响数据面,
        // cancel_job 只是让它更早止步。
        let watchman_state = state.clone();
        let watchman_id = id.clone();
        let watchman_from = from.to_string();
        tokio::spawn(async move {
            // Err 口径同 download 哨(spawn/插入竞窗早到即刻返回)
            let Ok(fin) = watchman_state.jobs().wait(&watchman_id).await else {
                return;
            };
            if fin.state != "cancelled" {
                return;
            }
            if let Err(e) = watchman_state
                .ctrl()
                .cancel_job(&watchman_state, &watchman_from, &watchman_id)
                .await
            {
                tracing::debug!(job = %watchman_id, detail = %e.rpc().1, "通知源端取消失败(忽略)")
            }
        });
        self.insert_job(
            id.clone(),
            JobHandle {
                core,
                pause,
                cancel_tx,
                task: Mutex::new(Some(driver)),
                round: Mutex::new(None),
            },
        );
        Ok(id)
    }

    /// 起一个 local_copy 作业(Task 7;M4b 移交债①收策略):本机复制/移动
    /// (spawn_blocking 跑 core fs 策略版 `copy_paths_with`/`move_entries_
    /// with`,与传输面同一匹配口径)。纯本地操作**不要求登录**;校验:
    /// paths 非空、dir 存在且为目录、dir 合法(绝对形态)、策略上限(与
    /// upload/download/remote_copy 同一收口——api 层 policy_from_params 已
    /// 校验,此处兜底保持「语义校验在 jobs 注册表收口」纪律)、预扫非空
    /// (同步反馈面)。
    pub(crate) async fn start_local_copy(
        &self,
        state: &Arc<DaemonState>,
        paths: Vec<PathBuf>,
        dir: &str,
        cut: bool,
        policy: TransferPolicy,
    ) -> Result<String, String> {
        if paths.is_empty() {
            return Err("paths 不能为空".into());
        }
        if !crate::flow::absolute_form(dir) {
            return Err(format!("dir 非法(须为绝对路径,Task 1 口径): {dir}"));
        }
        check_policy_limits(&policy)?;
        // 目录存在性检查 + 预扫:全树 rel(只读元数据)——files/bytes 总数
        // 与完成补齐表(都是阻塞调用,落线程池,直跑占死 async worker;
        // 目录缺失的业务文案原样保留)
        let scan_paths = paths.clone();
        let scan_dir = dir.to_string();
        let rels = tokio::task::spawn_blocking(move || -> Result<Vec<(String, bool, u64)>, String> {
            if !std::path::Path::new(&scan_dir).is_dir() {
                return Err(format!("目标目录不存在或不是目录: {scan_dir}"));
            }
            Ok(crate::ops::local_tree_rels(&scan_paths))
        })
        .await
        .map_err(|e| format!("扫描任务失败: {e}"))??;
        let mut rel_sizes = HashMap::new();
        let mut files_total = 0u64;
        let mut bytes_total = 0u64;
        for (rel, is_dir, size) in &rels {
            if !is_dir {
                files_total += 1;
                bytes_total += *size;
                rel_sizes.insert(rel.clone(), *size);
            }
        }
        if files_total == 0 {
            return Err("无可复制内容(输入缺失、空目录或全为符号链接)".into());
        }
        let src = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(", ");
        let meta = JobMeta {
            files_total,
            bytes_total,
            rel_sizes,
            src,
            dst: dir.to_string(),
        };

        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        let pause = PauseGate::new();
        let core = Arc::new(JobCore::new(
            id.clone(),
            "local_copy",
            meta,
            state.events(),
            self.count.clone(),
        ));
        {
            let m = lock_ok(&core.m);
            core.emit_locked(&m);
        }
        // 块内 shadow 同 upload(dir 为借用形参,先定形 owned)。
        let driver = {
            let core = core.clone();
            let pause = pause.clone();
            let dir = dir.to_string();
            tokio::spawn(supervise_driver(
                core.clone(),
                async move {
                    local_copy_driver(
                        core,
                        LocalCopyJob {
                            paths,
                            dir,
                            cut,
                            policy,
                        },
                        pause,
                    )
                    .await
                },
            ))
        };
        self.insert_job(
            id.clone(),
            JobHandle {
                core,
                pause,
                cancel_tx,
                task: Mutex::new(Some(driver)),
                round: Mutex::new(None),
            },
        );
        Ok(id)
    }

    /// 注册匿名 incoming 卡(Task 7,常驻接收循环调用):ConnectRequest 不
    /// 带 job_id(legacy send / 对端无任务面)→ 建 kind="incoming" 卡并绑定
    /// 轮,让每笔传输都有可见卡片。计数面同 download(远端数据,总数未知,
    /// RecvProgress 逐步填)。
    pub(crate) fn register_incoming(
        &self,
        state: &Arc<DaemonState>,
        from: &str,
        dir: &Path,
    ) -> String {
        let meta = JobMeta {
            files_total: 0,
            bytes_total: 0,
            rel_sizes: HashMap::new(),
            src: from.to_string(),
            dst: dir.to_string_lossy().into_owned(),
        };
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        let pause = PauseGate::new();
        let core = Arc::new(JobCore::new(
            id.clone(),
            "incoming",
            meta,
            state.events(),
            self.count.clone(),
        ));
        {
            let m = lock_ok(&core.m);
            core.emit_locked(&m);
        }
        // 无驱动任务:轮的收口(resident loop)即作业收口;cancel/暂停经
        // round 槽位直达该轮。
        self.insert_job(
            id.clone(),
            JobHandle {
                core,
                pause,
                cancel_tx,
                task: Mutex::new(None),
                round: Mutex::new(None),
            },
        );
        id
    }

    /// 接收进度路由(Task 7):RecvProgress → 绑定作业的聚合通道。作业不
    /// 存在(迟到/防御)静默忽略——进度回调只做毫秒级账,不报错。
    pub(crate) fn track_recv(&self, job_id: &str, p: &RecvProgress<'_>) {
        let core = lock_ok(&self.map).get(job_id).map(|h| h.core.clone());
        if let Some(core) = core {
            core.track_recv(p);
        }
    }

    /// 轮终局收口(Task 7,常驻接收循环调用):绑定下载与匿名卡共用——
    /// done/failed 经 transition 落定(终态粘滞使与 cancel 的竞速无害)。
    /// 作业不存在 → false(调用方忽略)。
    pub(crate) fn finish_incoming(&self, job_id: &str, state: &str, error: Option<String>) -> bool {
        let core = lock_ok(&self.map).get(job_id).map(|h| h.core.clone());
        match core {
            Some(core) => {
                core.finish(state, error);
                true
            }
            None => false,
        }
    }

    /// 全量快照(transfer.list 数据源),按序号升序:排序键见 [`job_seq`]。
    pub(crate) fn snapshot(&self) -> Vec<JobSnap> {
        let map = lock_ok(&self.map);
        let mut out: Vec<(u64, JobSnap)> = map
            .iter()
            .map(|(id, h)| (job_seq(id), h.core.snap()))
            .collect();
        out.sort_by_key(|(n, _)| *n);
        out.into_iter().map(|(_, s)| s).collect()
    }

    /// 暂停/恢复(false = resume):置任务暂停门 + 可见状态翻转;直达绑定的
    /// 接收轮门(Task 7 download 侧)。缺失或已终态 → false(不可暂停)。
    pub(crate) fn set_paused(&self, id: &str, paused: bool) -> bool {
        let core = {
            let map = lock_ok(&self.map);
            let Some(h) = map.get(id) else {
                return false;
            };
            if h.core.is_terminal() {
                return false;
            }
            // remote_copy 无本机数据面(暂停门没有消费方):放行只会翻转可见
            // 状态而 A→B 直传照跑——「状态 paused 实际在传」的假象比拒绝更
            // 糟,按不可暂停拒(false 面,与缺失/终态同一反馈;RPC 文案合并
            // 是接受的粗粒度)。
            if h.core.kind == "remote_copy" {
                return false;
            }
            h.pause.set(paused);
            if let Some(r) = lock_ok(&h.round).as_ref() {
                r.pause.set(paused); // 接收轮同门:双侧一起挂起/放开
            }
            h.core.clone()
        };
        core.transition_paused(paused);
        true
    }

    /// 取消:置 watch + 终态落定 cancelled + abort 驱动任务(语义见模块
    /// 文档;绑定接收轮若有,取消一并直达)。缺失或已终态 → false。
    pub(crate) fn cancel(&self, id: &str) -> bool {
        // 延迟初始化 + 块内赋值:JoinHandle 是 owned 值,块尾临时守卫不外泄
        let task: Option<JoinHandle<()>>;
        {
            let map = lock_ok(&self.map);
            let Some(h) = map.get(id) else {
                return false;
            };
            if h.core.is_terminal() {
                return false;
            }
            // 同一临界区内完成观察面三连:流内取消 watch、终态落定+事件、
            // 接收轮取消直达——abort 放到锁外(杀任务是纯同步快操作,但
            // 保持锁内零副作用原则)
            let _ = h.cancel_tx.send(true);
            if let Some(r) = lock_ok(&h.round).as_ref() {
                let _ = r.cancel.send(true);
            }
            h.core.transition("cancelled", None);
            task = lock_ok(&h.task).take();
        }
        if let Some(task) = task {
            task.abort(); // 连 run_send_with 重试循环一并杀,不复活
        }
        true
    }

    /// 终态长轮询(notify 唤醒)。不存在/已驱逐 id → 立即 `Err`(报文
    /// 「作业不存在: <id>」),绝不长轮询(M4a-1:终态保留上限引入驱逐后
    /// 存在性不再稳定,防御面由「合成 failed 快照」改为显式报错)。已入
    /// 轮的 waiter 手持 JobCore Arc,即便作业随后被逐也能等到其终态。
    pub(crate) async fn wait(&self, id: &str) -> Result<JobSnap, String> {
        let Some(core) = lock_ok(&self.map).get(id).map(|h| h.core.clone()) else {
            return Err(format!("作业不存在: {id}"));
        };
        loop {
            // enable 先于查终态:Notified 的登记序闭合「翻转与登记竞速」的
            // 漏唤醒窗口(见 tokio Notify 文档的循环用法)
            let notified = core.notify.notified();
            tokio::pin!(notified);
            let _ = notified.as_mut().enable();
            let snap = core.snap();
            if is_terminal_state(&snap.state) {
                return Ok(snap);
            }
            notified.await;
        }
    }

    /// resident 接收轮绑定(Task 7 接线 + 终审修复三分支):job_id 存在
    /// **且为可绑定的来件作业**(download = 操作端预注册的下载卡;incoming
    /// = 本机刚注册的匿名卡)**且非终态**即占槽。普通 upload 永不作为绑定
    /// 目标——下载 id 自带操作端命名空间(`<opDev>:<seq>`,见
    /// `start_download`)后,跨设备/跨操作端的同名序号不再可能命中本地卡;
    /// 本守卫兜住旧版裸数字 id 的残余面。重复绑定后写者胜(下载轮代际更替
    /// 重绑的自然覆盖)。
    ///
    /// 绑定纪律(终审 Important#1):
    /// - **暂停种子**:绑定即把作业级暂停门现值抄进轮门。暂停可能落在轮
    ///   装配前(orchestrate/打洞/重试间隙——轮尚未绑定,作业级门无消费
    ///   方,下载驱动只读它一次 `begin_running`);不抄即静默丢失:新轮
    ///   全新门 = 全速传输 + 卡片 paused、bytes_done 可见爬升的假态。
    /// - **终态拒绝**:已终态(典型 = 取消后仍在途的 ConnectRequest)不
    ///   占槽,并驱动该轮取消 watch 让轮死亡——已取消下载的数据不得落地;
    ///   返回 [`BindOutcome::RefusedTerminal`] 让调用方弃轮(不得降级匿名
    ///   卡,降级数据照样落地)。
    pub(crate) fn bind_incoming(&self, job_id: &str, round: RoundHandle) -> BindOutcome {
        let map = lock_ok(&self.map);
        match map.get(job_id) {
            Some(h) if matches!(h.core.kind, "download" | "incoming") => {
                if h.core.is_terminal() {
                    // 取消/完成/失败后的迟到绑定:驱动轮取消 watch(消费方
                    // 在传输 select 处),数据不落地
                    let _ = round.cancel.send(true);
                    return BindOutcome::RefusedTerminal;
                }
                if h.pause.paused() {
                    round.pause.set(true); // 暂停种子:作业级门 → 轮门
                }
                *lock_ok(&h.round) = Some(round);
                BindOutcome::Bound
            }
            _ => BindOutcome::Miss,
        }
    }
}

// ---------- 驱动监督(M4a-2) ----------

/// 驱动任务监督网:四个驱动(upload/download/remote_copy/local_copy)在
/// Ok/Err 两路都会 `core.finish(...)` 才返回,本包装只接**意外退出**——
/// panic(unwind)被 `catch_unwind` 捕获后,作业尚未终态即落 failed(文案
/// 钉死「内部错误: 传输驱动异常退出」),否则用户侧卡片永远 running 只能
/// 靠取消救。已终态(取消/完成竞速先到)不改写;正常路径透传无附加开销;
/// JoinHandle 形态不变(仍 JoinHandle<()>,返回 Ok);abort 取消路径不经
/// 此(挂起点直接杀任务,不产生 unwind)。panic 详情由默认 hook 照常落
/// stderr 留痕,此处不重复记日志。
async fn supervise_driver(core: Arc<JobCore>, fut: impl std::future::Future<Output = ()>) {
    use std::panic::AssertUnwindSafe;
    let caught = futures_util::FutureExt::catch_unwind(AssertUnwindSafe(fut)).await;
    if caught.is_err() && !core.is_terminal() {
        core.transition("failed", Some("内部错误: 传输驱动异常退出".into()));
    }
}

// ---------- upload 驱动 ----------

/// upload 驱动任务的载荷(spawn 边界一次性搬移)。
struct UploadJob {
    files: Vec<PathBuf>,
    to: String,
    dir: String,
    policy: TransferPolicy,
    cut_src: bool,
}

/// upload 驱动:跑 run_send_with(SendSpec 定向 + ctl 暂停/取消/进度),
/// 进度回调进注册表(聚合计数 + 速度 EMA + 节流事件);成功且 cut → 本地
/// 删源(先于 done 落定);终态按结果落定(取消竞速时粘滞,先到者胜)。
async fn upload_driver(
    state: Arc<DaemonState>,
    core: Arc<JobCore>,
    job: UploadJob,
    pause: Arc<PauseGate>,
    cancel_rx: watch::Receiver<bool>,
) {
    core.begin_running(pause.paused());
    let Some(session) = state.session() else {
        core.finish("failed", Some("未登录,作业无法发起".into()));
        return;
    };
    let spec = crate::flow::SendSpec {
        target_dir: job.dir.clone(),
        job_id: core.id.clone(),
        policy: job.policy.clone(),
    };
    // 进度回调:引擎 SendProgress → 注册表聚合/节流事件(克隆 Arc 进闭包,
    // out_report 另行借用 core 打日志,两者不冲突)
    let tracking = core.clone();
    let progress = Arc::new(
        move |p: &SendProgress<'_>| -> () { tracking.track_send(p) },
    );
    let ctl = crate::flow::SendCtl {
        pause: Some(pause),
        cancel: Some(cancel_rx),
        progress: Some(progress),
    };
    let home = state.home();
    let opts = state.send_opts();
    // 遗留 send_log 事件不进作业面(event.transfer 是作业的进度契约);
    // 清单/完成行落 tracing 供排障。
    let out_report = |line: &str| tracing::info!(job = %core.id, "{line}");
    let result = crate::flow::run_send_with(
        &session,
        &home,
        state.hub(),
        &job.files,
        &job.to,
        &spec,
        &out_report,
        &opts,
        &ctl,
    )
    .await;
    match result {
        Ok(()) => {
            if job.cut_src {
                cut_finalize(&job.files).await;
            }
            core.finish("done", None);
        }
        Err(e) => core.finish("failed", Some(format!("{e:#}"))),
    }
}

/// cut 收尾:驱动成功后本地删源(delete_paths 回收优先,尽力而为——
/// 失败仅留痕不翻终态;删源先于 done 落定,wait 返回时磁盘事实已定)。
async fn cut_finalize(files: &[PathBuf]) {
    let paths: Vec<String> = files
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    match tokio::task::spawn_blocking(move || mtdss_core::fs::delete_paths(&paths, false)).await {
        Ok(Ok((deleted, recycled, failed))) if failed.is_empty() => {
            tracing::info!(deleted, recycled, "cut 删源完成(回收优先)")
        }
        Ok(Ok((deleted, recycled, failed))) => {
            tracing::warn!(deleted, recycled, ?failed, "cut 删源部分失败(尽力回收)")
        }
        Ok(Err(e)) => tracing::warn!(%e, "cut 删源失败(源保留)"),
        Err(e) => tracing::warn!(%e, "cut 删源任务收尾失败(源保留)"),
    }
}

// ---------- download 驱动(Task 7) ----------

/// download 驱动任务的载荷(spawn 边界一次性搬移)。
struct DownloadJob {
    /// 源设备(orchestrate 的 ctrl 对端;cancel_job 通知同一对端)。
    src_device: String,
    /// 远端绝对路径(源端视角)。
    paths: Vec<String>,
    /// 本机落盘目录(已校验绝对形态)。
    dir: String,
    policy: TransferPolicy,
    /// 成功后对源端 ctrl delete 源路径(回收优先,尽力而为)。
    cut: bool,
}

/// download 驱动的源端作业态轮询间隔:轮失败后的源端重试间隙里,本地作业
/// 保持 running 等重来的 ConnectRequest;终局失败(源端重试耗尽/协议终局/
/// 查询持续不可达)由本探测收口。3s = 失败感知延迟与 ctrl 请求开销的折中
/// (池内连接复用,单次请求毫秒级)。
const DOWNLOAD_SOURCE_POLL: Duration = Duration::from_secs(3);

/// download 驱动:orchestrate 到源设备(源端起同 id upload 驱动)→ 等
/// 本地作业终态(轮成功由 resident loop 落 done;取消由注册表 abort 本任
/// 务)→ 期间周期 job_query 探测源端作业态:源端 failed/missing → 本地
/// failed;源端 cancelled → 本地 cancelled。done 且 cut → 对源端 ctrl
/// delete 源路径(回收优先,尽力而为,失败仅留痕不翻终态)。
async fn download_driver(state: Arc<DaemonState>, core: Arc<JobCore>, job: DownloadJob, pause: Arc<PauseGate>) {
    core.begin_running(pause.paused());
    let ctrl = state.ctrl();
    // orchestrate:to_device = 自己(源端把数据传回本机)
    let me = match state.self_device_id() {
        Some(me) => me,
        None => {
            core.finish("failed", Some("未登录,作业无法发起".into()));
            return;
        }
    };
    let resp = match ctrl
        .orchestrate(
            &state,
            &job.src_device,
            &core.id,
            &me,
            &job.paths,
            &job.dir,
            &job.policy,
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let (_, msg) = e.rpc();
            core.finish("failed", Some(msg));
            return;
        }
    };
    if !resp.accepted {
        core.finish("failed", Some(format!("源端拒绝编排: {}", resp.detail)));
        return;
    }
    // 源端作业态轮询的注入面(评审 Important#2 可测缝):生产 = ctrl
    // job_query,查询失败映射 None(忽略,下轮再试);测试注入合成态。
    let poll_ctrl = ctrl.clone();
    let poll_state = state.clone();
    let poll_id = core.id.clone();
    let poll_src = job.src_device.clone();
    let mut poll = move || {
        let (ctrl, st, id, src) =
            (poll_ctrl.clone(), poll_state.clone(), poll_id.clone(), poll_src.clone());
        async move { ctrl.job_query(&st, &src, &id).await.ok().map(|q| (q.state, q.error)) }
    };
    download_wait_terminal(
        &state,
        core,
        &job,
        DOWNLOAD_SOURCE_POLL,
        DOWNLOAD_SOURCE_DONE_GRACE,
        SOURCE_POLL_FAIL_BOUND,
        &mut poll,
    )
    .await;
}

/// 源端作业「已完成」后本地仍未收口的宽限窗:源端发完到接收端
/// finalize/EOF 舞步收尾有正常时差,窗内等本地自然 done;超窗仍 running
/// = 本地轮已死(收尾错误/取消竞速,再无 ConnectRequest 可救)——按失败
/// 收口,杜绝 `wait_job` 无界挂起(评审 Important#2)。
const DOWNLOAD_SOURCE_DONE_GRACE: Duration = Duration::from_secs(10);

/// download 驱动的终局等待(编排接受后):本地作业终态(轮成功/取消)即
/// 出;周期轮询源端作业态——failed/cancelled/missing 即时映射本地终态;
/// **done 不即时映射**(源端发完 ≠ 收端已收口),见 `done_grace`。查询
/// 持续不可达(`fail_bound` 内无一次成功查询)→ 失败收口(对端真下线
/// 不悬挂;单次瞬时失败不烧计数,任一成功查询归零;取消经 waiter 臂/
/// abort 即时胜出)。本地 done 且 cut → 对源端 ctrl delete 源路径(回收
/// 优先,尽力而为)。
/// `poll` 为源端作业态查询的注入缝,返回 `None` = 查询失败(单次忽略,
/// 持续超 `fail_bound` 失败收口)。
#[allow(clippy::too_many_arguments)] // 注入缝全参数面(poll_tick/done_grace/fail_bound 皆测试可调)
async fn download_wait_terminal<F, Fut>(
    state: &Arc<DaemonState>,
    core: Arc<JobCore>,
    job: &DownloadJob,
    poll_tick: Duration,
    done_grace: Duration,
    fail_bound: Duration,
    mut poll: F,
) where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<(String, String)>>,
{
    let mut ticker = tokio::time::interval(poll_tick);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let waiter = state.jobs().wait(&core.id);
    tokio::pin!(waiter);
    let mut done_since: Option<Instant> = None;
    // 查询连续失败计时:首败记时,任一成功查询归零;超界 → 失败收口
    let mut fail_since: Option<Instant> = None;
    loop {
        tokio::select! {
            fin = &mut waiter => {
                // Err = 无此作业/已驱逐(防御面:活跃作业不驱逐,驱动等自身
                // 作业正常不可达)——与终态同口径收口退出,cut 收尾仅 done 执行
                if matches!(&fin, Ok(s) if s.state == "done") && job.cut {
                    cut_finalize_remote(state, &job.src_device, &job.paths).await;
                }
                return;
            }
            _ = ticker.tick() => {
                match poll().await {
                    Some((q, err)) => {
                        // 任一成功查询:连续失败计时归零(单次瞬时失败不计)
                        fail_since = None;
                        if q == "failed" {
                            let detail = if err.is_empty() {
                                "源端作业失败".to_string()
                            } else {
                                format!("源端作业失败: {err}")
                            };
                            core.finish("failed", Some(detail));
                            return;
                        }
                        if q == "cancelled" {
                            core.finish("cancelled", None);
                            return;
                        }
                        // missing:源端无此作业(重启/登出拆卸)——永不完成,按失败收口
                        if q == "missing" {
                            core.finish("failed", Some("源端已无此作业(设备可能重启)".into()));
                            return;
                        }
                        if q == "done" {
                            // 首见记时,宽限内等本地自然 done(waiter 臂收口,终态
                            // 粘滞使两侧竞速无害);超窗仍 running = 本地轮已死。
                            let since = *done_since.get_or_insert_with(Instant::now);
                            if since.elapsed() >= done_grace {
                                core.finish(
                                    "failed",
                                    Some(
                                        "源端已完成,但本地接收未在宽限期内收口(本地轮收尾失败或已取消)"
                                            .into(),
                                    ),
                                );
                                return;
                            }
                        }
                        // queued/running:继续等
                    }
                    None => {
                        // 查询失败(打洞中断/对端不可达):单次瞬时忽略;持续超
                        // 界 = 对端真下线,失败收口(永不下线的 running 比显式
                        // 失败更糟,评审 Important)。
                        let since = *fail_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= fail_bound {
                            core.finish(
                                "failed",
                                Some(format!(
                                    "源端持续不可达(>{}s),已停止等待——可重试",
                                    fail_bound.as_secs()
                                )),
                            );
                            return;
                        }
                    }
                }
            }
        }
    }
}

/// download 的 cut 收尾:本地 done 后对源端 ctrl delete 源路径(force=false,
/// 回收优先;尽力而为——失败仅留痕不翻终态,done 已落定)。
async fn cut_finalize_remote(state: &Arc<DaemonState>, device: &str, paths: &[String]) {
    match state.ctrl().delete(state, device, paths, false).await {
        Ok(r) => {
            if r.failed.is_empty() {
                tracing::info!(deleted = r.deleted, recycled = r.recycled, "download cut 收尾完成(源端回收删除)")
            } else {
                tracing::warn!(?r.failed, "download cut 收尾部分失败(尽力回收)")
            }
        }
        Err(e) => tracing::warn!(e = %e.rpc().1, "download cut 收尾失败(源端路径保留)"),
    }
}

// ---------- remote_copy 驱动(Task 8:A→B 编排) ----------

/// remote_copy 驱动任务的载荷(spawn 边界一次性搬移)。
struct RemoteCopyJob {
    /// 源设备(orchestrate/cancel_job/job_query/cut 收尾的 ctrl 对端;数据
    /// 从它读出)。
    from: String,
    /// 远端绝对路径(from 设备视角)。
    paths: Vec<String>,
    /// 目标设备(数据直落它,不经操作端)。
    to: String,
    /// 目标目录(to 设备视角,已校验绝对形态)。
    dir: String,
    policy: TransferPolicy,
    /// 成功后对源端 ctrl delete 源路径(回收优先,尽力而为)。
    cut: bool,
}

/// remote_copy 的源端作业态轮询间隔(brief 钉死 1.5s):本端卡片可见期间
/// 持续探测 from 端作业态,终局即时映射本端终态。池内连接复用,单次请求
/// 毫秒级;1.5s = 终局感知延迟与 ctrl 请求开销的折中。
const REMOTE_COPY_SOURCE_POLL: Duration = Duration::from_millis(1500);

/// remote_copy 驱动:预扫(① from 端逐源 stat——源缺失在编排前快败;②
/// to 端目标目录 list——目录缺失 = 无冲突,截断按顶层可见集比对 + 警示;
/// 命中同名仅留痕,策略以调用方下发表为准)→ orchestrate 发给 **from 设备**
/// (to_device = to;from 起回显 job_id 的 upload 驱动,ConnectRequest 定向
/// to)→ 1.5s 轮询 from 端 job_query 至终态并即时映射本端终态;done 且
/// cut → 对 from 端 ctrl delete 源路径(回收优先,尽力而为)。
async fn remote_copy_driver(state: Arc<DaemonState>, core: Arc<JobCore>, job: RemoteCopyJob, pause: Arc<PauseGate>) {
    core.begin_running(pause.paused());
    let ctrl = state.ctrl();
    // 预扫 ①:from 端逐源 stat(顶层名 + is_dir;已知 V1 限制:冲突策略
    // 顶层粒度,嵌套同名交接收端引擎安全网)
    let mut src_names: Vec<(String, bool)> = Vec::with_capacity(job.paths.len());
    for p in &job.paths {
        match ctrl.stat(&state, &job.from, p).await {
            Ok(d) => src_names.push((d.name, d.is_dir)),
            Err(e) => {
                let (_, msg) = e.rpc();
                core.finish("failed", Some(format!("源端预扫失败({p}): {msg}")));
                return;
            }
        }
    }
    // 预扫 ②:to 端目标目录 list。目录缺失(NOT_FOUND)= 无冲突——接收端
    // 会按定向目录建出,与 upload 的目录语义一致。
    let (entries, truncated) = match ctrl.list(&state, &job.to, &job.dir).await {
        Ok(out) => (out.entries, out.truncated),
        Err(CtrlErr::Peer { code, .. }) if code == "NOT_FOUND" => (Vec::new(), false),
        Err(e) => {
            let (_, msg) = e.rpc();
            core.finish("failed", Some(format!("目标目录预扫失败: {msg}")));
            return;
        }
    };
    if truncated {
        tracing::warn!(dir = %job.dir, "目标目录列表截断:预扫按顶层可见集比对(V1 顶层粒度策略边界)");
    }
    let conflicts: Vec<&str> = src_names
        .iter()
        .filter(|(name, _)| entries.iter().any(|e| e.name == *name))
        .map(|(name, _)| name.as_str())
        .collect();
    if !conflicts.is_empty() {
        tracing::info!(?conflicts, "remote_copy 预扫命中目标同名条目(策略以调用方下发表为准)");
    }
    // orchestrate:发给 **from 设备**,to_device = to(A 的 upload 驱动把
    // 数据直传给 B,数据不经本机落地)
    let resp = match ctrl
        .orchestrate(
            &state,
            &job.from,
            &core.id,
            &job.to,
            &job.paths,
            &job.dir,
            &job.policy,
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let (_, msg) = e.rpc();
            core.finish("failed", Some(msg));
            return;
        }
    };
    if !resp.accepted {
        core.finish("failed", Some(format!("源端拒绝编排: {}", resp.detail)));
        return;
    }
    // 源端作业态轮询的注入面(评审 Important#2 同款可测缝):生产 = ctrl
    // job_query,查询失败映射 None(忽略,下轮再试);测试注入合成态。
    let poll_ctrl = ctrl.clone();
    let poll_state = state.clone();
    let poll_id = core.id.clone();
    let poll_from = job.from.clone();
    let mut poll = move || {
        let (ctrl, st, id, from) =
            (poll_ctrl.clone(), poll_state.clone(), poll_id.clone(), poll_from.clone());
        async move { ctrl.job_query(&st, &from, &id).await.ok().map(|q| (q.state, q.error)) }
    };
    remote_copy_wait_terminal(
        &state,
        core,
        &job,
        REMOTE_COPY_SOURCE_POLL,
        SOURCE_POLL_FAIL_BOUND,
        &mut poll,
    )
    .await;
}

/// remote_copy 的终局等待(from 端编排接受后):周期轮询 from 端作业态并
/// **即时映射**本端终态——done 无宽限窗(与 download 不同:本机无数据面,
/// from 端驱动收口 = to 端已 finalize,不存在收端时差);failed/cancelled/
/// missing 同 download 口径。查询持续不可达(`fail_bound` 内无一次成功查
/// 询)→ 失败收口(对端真下线不悬挂;单次瞬时失败不烧计数,任一成功查询
/// 归零;取消经 waiter 臂/abort 即时胜出)。done 且 cut → 对 from 端 ctrl
/// delete 源路径(回收优先,尽力而为;删源先于 done 落定)。本地 cancel
/// 落定即出(waiter 臂,防御性:registry.cancel 同时在 abort 本任务,双臂
/// 竞速无害——终态粘滞)。
/// `poll` 为源端作业态查询的注入缝(评审 Important#2 同款),返回 None =
/// 查询失败(单次忽略,持续超 `fail_bound` 失败收口)。
async fn remote_copy_wait_terminal<F, Fut>(
    state: &Arc<DaemonState>,
    core: Arc<JobCore>,
    job: &RemoteCopyJob,
    poll_tick: Duration,
    fail_bound: Duration,
    mut poll: F,
) where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<(String, String)>>,
{
    let mut ticker = tokio::time::interval(poll_tick);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let waiter = state.jobs().wait(&core.id);
    tokio::pin!(waiter);
    // 查询连续失败计时:首败记时,任一成功查询归零;超界 → 失败收口
    // (与 download_wait_terminal 同一兜底语义,评审 Important)
    let mut fail_since: Option<Instant> = None;
    loop {
        tokio::select! {
            // 本地终态先行(cancel 竞速;终态粘滞使双臂无害)
            _ = &mut waiter => return,
            _ = ticker.tick() => {
                match poll().await {
                    Some((q, err)) => {
                        // 任一成功查询:连续失败计时归零(单次瞬时失败不计)
                        fail_since = None;
                        if q == "done" {
                            if job.cut {
                                cut_finalize_remote(state, &job.from, &job.paths).await;
                            }
                            core.finish("done", None);
                            return;
                        }
                        if q == "failed" {
                            let detail = if err.is_empty() {
                                "源端作业失败".to_string()
                            } else {
                                format!("源端作业失败: {err}")
                            };
                            core.finish("failed", Some(detail));
                            return;
                        }
                        if q == "cancelled" {
                            core.finish("cancelled", None);
                            return;
                        }
                        // missing:from 端无此作业(重启/登出拆卸)——永不完成,按失败收口
                        if q == "missing" {
                            core.finish("failed", Some("源端已无此作业(设备可能重启)".into()));
                            return;
                        }
                        // queued/running:继续等
                    }
                    None => {
                        // 查询失败(打洞中断/对端不可达):单次瞬时忽略;持续超
                        // 界 = 对端真下线,失败收口(永不下线的 running 比显式
                        // 失败更糟,评审 Important)。
                        let since = *fail_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= fail_bound {
                            core.finish(
                                "failed",
                                Some(format!(
                                    "源端持续不可达(>{}s),已停止等待——可重试",
                                    fail_bound.as_secs()
                                )),
                            );
                            return;
                        }
                    }
                }
            }
        }
    }
}

// ---------- local_copy 驱动(Task 7) ----------

/// local_copy 驱动任务的载荷(spawn 边界一次性搬移)。
struct LocalCopyJob {
    /// 本机源路径(顶层)。
    paths: Vec<PathBuf>,
    /// 本机目标目录(已校验存在且为目录)。
    dir: String,
    /// true = 移动快路(逐顶层条目 rename);false = fs::copy_paths 复制。
    cut: bool,
    /// 冲突策略(M4b 移交债①):skip 命中整项跳过(计入 skipped),与
    /// 传输面同口径。
    policy: TransferPolicy,
}

/// local_copy 驱动:spawn_blocking 跑 core fs 策略版(整树复制/回收删除级
/// 阻塞调用直跑会占死异步 worker,与 ctrl 面同一收口纪律);起步先过暂停
/// 门(门上等恢复,cancel 由注册表 abort 本任务兜底——阻塞中的 fs 操作不可
/// 中断,abort 落在 await 点,复制线程自然跑完被弃)。cut=false →
/// `copy_paths_with`,true → `move_entries_with`;终局:outcome.failed 非
/// 空 → failed 携带失败表明细(沿现形态),空 → mark_all_done 补齐计数面
/// 再 done;skip 命中数终局前回填(`record_skipped`,终态帧与快照自带)。
async fn local_copy_driver(core: Arc<JobCore>, job: LocalCopyJob, pause: Arc<PauseGate>) {
    core.begin_running(pause.paused());
    if pause.paused() {
        pause.wait().await;
    }
    let paths = job.paths.clone();
    let dir = job.dir.clone();
    let cut = job.cut;
    let policy = job.policy.clone();
    // 统一结果面 (成功计数, 逐条失败表, skip 命中数):copy/move 两路同形,
    // 终局判定不分支(copy 沿 legacy「任一条失败整批 Err」,失败表恒空——
    // 失败经 Ok(Err) 臂;move 逐对失败表不弃批,失败经 failed 表臂)。
    let outcome = tokio::task::spawn_blocking(move || -> Result<(usize, Vec<mtdss_core::fs::PathOpFailure>, u64), mtdss_core::fs::FsError> {
        if cut {
            // 移动快路:逐顶层条目 rename 入目标目录(文件覆盖既有文件、
            // 目录撞既有名逐对失败,与 rename 原生语义一致;跨卷回退暂存
            // 复制 + 源永久删除,fs 层透明处理)。skip 命中整对跳过,
            // overwrite 命中 rename 直通(判定键 = 目标 to 串,fs 层口径)。
            let mut pairs: Vec<(PathBuf, PathBuf)> = Vec::with_capacity(paths.len());
            for p in &paths {
                let Some(name) = p.file_name() else {
                    return Err(mtdss_core::fs::FsError::InvalidPath(format!(
                        "移动输入无文件名(卷根等): {}",
                        p.display()
                    )));
                };
                let to = std::path::Path::new(&dir).join(name);
                pairs.push((p.clone(), to));
            }
            let out = mtdss_core::fs::move_entries_with(&pairs, &policy)?;
            Ok((out.moved.len(), out.failed, out.skipped.len() as u64))
        } else {
            // 复制:skip 命中整项跳过不落盘,overwrite 命中清文件型占用后
            // 原位落位(判定键 = 目标目录拼名,fs 层口径);未命中沿 legacy
            // 自动改名。
            let out = mtdss_core::fs::copy_paths_with(
                &paths,
                std::path::Path::new(&dir),
                &policy,
            )?;
            Ok((out.copied.len(), out.failed, out.skipped.len() as u64))
        }
    })
    .await;
    match outcome {
        Ok(Ok((n, failed, skipped))) => {
            core.record_skipped(skipped);
            if failed.is_empty() {
                core.mark_all_done();
                core.finish("done", None);
            } else {
                let detail: Vec<String> = failed
                    .iter()
                    .map(|f| format!("{}({}: {})", f.path, f.code, f.message))
                    .collect();
                core.finish(
                    "failed",
                    Some(format!("移动 {n} 项成功,{} 项失败: {}", failed.len(), detail.join("; "))),
                );
            }
        }
        Ok(Err(e)) => core.finish("failed", Some(e.to_string())),
        Err(e) => core.finish("failed", Some(format!("本地复制任务收尾失败: {e}"))),
    }
}

// ---------- 单元测试(评审 Critical#1 / Important#2 回归) ----------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Session;

    /// 换装 std panic hook 的测试共用此串行化锁:hook 是进程全局,并行
    /// 测试线程交错 take/set 会互相踩(把别人的 hook 当 prev 恢复)。
    /// 守卫在 take_hook 前取,保持到末次 set_hook 恢复之后。
    static PANIC_HOOK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 测试态:独立临时 home + 已登录会话(device_id 即参数;home 生命周期
    /// 到函数尾即弃,后续仅用内存面)。
    fn test_state(device_id: &str) -> Arc<DaemonState> {
        let home = tempfile::tempdir().unwrap();
        let (events, _) = tokio::sync::broadcast::channel(16);
        let state = DaemonState::load(
            home.path(),
            home.path().join("inbox"),
            Default::default(),
            Default::default(),
            events,
        )
        .unwrap();
        state
            .login(Session {
                server: "http://test".into(),
                jwt: "jwt".into(),
                refresh: "refresh".into(),
                device_id: device_id.into(),
                ca: None,
            })
            .unwrap();
        Arc::new(state)
    }

    /// 平台中立的绝对形态测试目录。
    fn abs_dir() -> String {
        if cfg!(windows) { "C:\\dst".into() } else { "/dst".into() }
    }

    /// 预置一个真实文件(start_upload/accept_orchestration 的预扫要求)。
    fn make_file() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.bin");
        std::fs::write(&f, b"data").unwrap();
        (dir, f)
    }

    /// 模块内直构 download 作业卡(测试注入:绕过 start_download 的驱动
    /// spawn 与离线编排),经 insert_job 注册进注册表供 `wait` 长轮询。
    fn make_download_card(state: &Arc<DaemonState>, seq: u64) -> (Arc<JobCore>, String) {
        let id = format!("{}:{}", state.self_device_id().unwrap(), seq);
        let meta = JobMeta {
            files_total: 0,
            bytes_total: 0,
            rel_sizes: HashMap::new(),
            src: "srcB:/x".into(),
            dst: "/dst".into(),
        };
        let core = Arc::new(JobCore::new(id.clone(), "download", meta, state.events(), state.jobs().count.clone()));
        {
            let m = lock_ok(&core.m);
            core.emit_locked(&m);
        }
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        state.jobs().insert_job(
            id.clone(),
            JobHandle {
                core: core.clone(),
                pause: PauseGate::new(),
                cancel_tx,
                task: Mutex::new(None),
                round: Mutex::new(None),
            },
        );
        (core, id)
    }

    /// make_download_card 的终态变体(M4a-1 驱逐面装配件):先落终态再
    /// 插入——驱逐只发生在插入时,候选必须以终态身份入场,条数/时点才
    /// 对得齐断言。
    fn make_terminal_card(state: &Arc<DaemonState>, seq: u64) -> String {
        let id = format!("{}:{}", state.self_device_id().unwrap(), seq);
        let meta = JobMeta {
            files_total: 0,
            bytes_total: 0,
            rel_sizes: HashMap::new(),
            src: "srcB:/x".into(),
            dst: "/dst".into(),
        };
        let core = Arc::new(JobCore::new(
            id.clone(),
            "download",
            meta,
            state.events(),
            state.jobs().count.clone(),
        ));
        core.finish("done", None);
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        state.jobs().insert_job(
            id.clone(),
            JobHandle {
                core,
                pause: PauseGate::new(),
                cancel_tx,
                task: Mutex::new(None),
                round: Mutex::new(None),
            },
        );
        id
    }

    fn download_job() -> DownloadJob {
        DownloadJob {
            src_device: "srcB".into(),
            paths: vec!["/x".into()],
            dir: "/dst".into(),
            policy: TransferPolicy::default(),
            cut: false,
        }
    }

    /// 托盘计数 watch(Task 13):建队 +1、终态 -1;非终态翻转不计数,
    /// 终态粘滞(迟到翻帧)不重复扣。
    #[tokio::test]
    async fn jobs_count_watch_tracks_active() {
        let state = test_state("opA");
        let rx = state.jobs().count_rx();
        assert_eq!(*rx.borrow(), 0, "空注册表计数为 0");
        let (core, _id) = make_download_card(&state, 1);
        assert_eq!(*rx.borrow(), 1, "建队 +1");
        core.transition("running", None);
        assert_eq!(*rx.borrow(), 1, "非终态翻转不计数");
        core.transition("done", None);
        assert_eq!(*rx.borrow(), 0, "终态落定 -1");
        core.transition("failed", Some("迟到错误".into()));
        assert_eq!(*rx.borrow(), 0, "终态粘滞:迟到翻帧不重复扣");
    }

    /// M4b Task 10(D12):`active_count` = 非终态作业数——paused 计入
    /// (无独立 stalled 态,stalled 即 running),终态不计。更新器空闲门
    /// (Task 11)的数据源语义在此钉死。
    #[tokio::test]
    async fn active_count_includes_paused_excludes_terminal() {
        let state = test_state("opA");
        assert_eq!(state.jobs().active_count(), 0, "空注册表计数为 0");
        let (core1, id1) = make_download_card(&state, 1);
        let (_core2, id2) = make_download_card(&state, 2);
        let _terminal = make_terminal_card(&state, 3); // 终态入场:+1 后即 -1
        assert_eq!(state.jobs().active_count(), 2, "两张活跃卡;终态卡不计");
        assert!(state.jobs().set_paused(&id1, true), "running 卡可暂停");
        assert_eq!(core1.snap().state, "paused");
        assert_eq!(
            state.jobs().active_count(),
            2,
            "paused 非终态,必须计入活跃(D12)"
        );
        assert!(state.jobs().cancel(&id2), "取消成功");
        assert_eq!(
            state.jobs().active_count(),
            1,
            "取消终态落定 -1,只剩 paused 一张"
        );
    }

    /// 构造一个未绑定轮的 RoundHandle(cancel watch + 新暂停门;进度回调
    /// 空实现)——bind 面单测的装配件。
    fn loose_round() -> (RoundHandle, watch::Receiver<bool>, Arc<PauseGate>) {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let gate = PauseGate::new(); // 生来即 Arc(跨任务共享门)
        let progress: Arc<dyn Fn(&RecvProgress<'_>) + Send + Sync> = Arc::new(|_| {});
        (
            RoundHandle {
                cancel: cancel_tx,
                pause: gate.clone(),
                progress,
            },
            cancel_rx,
            gate,
        )
    }

    /// 终审修复 Important#1(暂停面):暂停落在轮装配/绑定**之前**
    /// (orchestrate/打洞/重试间隙的等价物——作业级门无消费方)时,
    /// bind_incoming 必须把作业级暂停门现值种子进轮门;未暂停作业绑定
    /// 不得引入暂停。
    #[tokio::test]
    async fn bind_incoming_seeds_job_pause_into_round_gate() {
        let state = test_state("opA");
        // 暂停先于绑定
        let (core, id) = make_download_card(&state, 1);
        core.begin_running(false);
        assert!(state.jobs().set_paused(&id, true), "running 卡可暂停");
        assert_eq!(core.snap().state, "paused");
        let (handle, _cancel_rx, round_gate) = loose_round();
        assert_eq!(state.jobs().bind_incoming(&id, handle), BindOutcome::Bound);
        assert!(round_gate.paused(), "绑定时必须把作业级暂停态抄进轮门(否则新轮全新门 = 暂停静默丢失)");
        // 对照:未暂停作业绑定不引入暂停
        let (core2, id2) = make_download_card(&state, 2);
        core2.begin_running(false);
        let (handle2, _cancel_rx2, round_gate2) = loose_round();
        assert_eq!(state.jobs().bind_incoming(&id2, handle2), BindOutcome::Bound);
        assert!(!round_gate2.paused(), "未暂停作业绑定不得引入暂停");
    }

    /// 终审修复 Important#1(终态面):已取消作业的迟到绑定必须拒绝
    /// (RefusedTerminal)并驱动该轮取消 watch——取消后仍在途的
    /// ConnectRequest 不得把已取消下载的数据带落地。
    #[tokio::test]
    async fn bind_incoming_refuses_terminal_and_drives_round_cancel() {
        let state = test_state("opA");
        let (core, id) = make_download_card(&state, 3);
        core.begin_running(false);
        assert!(state.jobs().cancel(&id), "取消成功");
        assert_eq!(core.snap().state, "cancelled");
        // ConnectRequest 在取消前已发出,此刻才到 bind
        let (handle, mut cancel_rx, _gate) = loose_round();
        assert_eq!(
            state.jobs().bind_incoming(&id, handle),
            BindOutcome::RefusedTerminal,
            "终态作业不得绑定"
        );
        assert!(*cancel_rx.borrow_and_update(), "拒绝绑定时必须驱动轮取消 watch(轮死亡,数据不落地)");
        // 未命中的 id 仍是 Miss(降级匿名卡语义不变)
        let (handle2, _cancel_rx2, _gate2) = loose_round();
        assert_eq!(
            state.jobs().bind_incoming(&format!("{}:99", state.self_device_id().unwrap()), handle2),
            BindOutcome::Miss
        );
    }

    /// 评审 Critical#1:download id 必须带操作端命名空间(`<dev>:<seq>`),
    /// 且同操作端连续 mint 序号单调。
    #[tokio::test]
    async fn download_id_is_operator_namespaced() {
        let state = test_state("opA");
        let dir = abs_dir();
        let id1 = state
            .jobs()
            .start_download(&state, "srcB", vec!["/x".into()], &dir, TransferPolicy::default(), false)
            .unwrap();
        let id2 = state
            .jobs()
            .start_download(&state, "srcB", vec!["/x".into()], &dir, TransferPolicy::default(), false)
            .unwrap();
        assert_eq!(id1, "opA:1", "download id 必须是 `<操作端>:<序号>`: {id1}");
        assert_eq!(id2, "opA:2", "同操作端序号必须单调: {id2}");
    }

    /// 评审 Critical#1:源端接受回显后,自身自增面不得撞回显键、不得覆盖
    /// 回显条目(驱动 JoinHandle 丢失即作业孤儿化)。
    #[tokio::test]
    async fn accept_echo_then_self_mint_no_clobber() {
        let op = test_state("opA");
        let src = test_state("srcB");
        let dir = abs_dir();
        let (_keep, f) = make_file();
        let f_str = f.to_string_lossy().into_owned();

        let id = op
            .jobs()
            .start_download(&op, "srcB", vec!["/x".into()], &dir, TransferPolicy::default(), false)
            .unwrap();
        assert_eq!(id, "opA:1");

        src.jobs()
            .accept_orchestration(&src, &id, "opA", std::slice::from_ref(&f_str), &dir, TransferPolicy::default())
            .await
            .unwrap();
        let minted = src
            .jobs()
            .start_upload(&src, vec![f.clone()], "peer", &dir, TransferPolicy::default(), false)
            .await
            .unwrap();

        let snaps = src.jobs().snapshot();
        let ids: Vec<&str> = snaps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids.len(), 2, "回显条目不得被自增条目覆盖: {ids:?}");
        assert!(ids.contains(&id.as_str()), "回显条目必须在场: {ids:?}");
        assert!(ids.contains(&minted.as_str()), "自增条目必须在场: {ids:?}");
        let echo = snaps.iter().find(|s| s.id == id).unwrap();
        assert_eq!(echo.kind, "upload", "回显条目面必须完好");
    }

    /// 评审 Critical#1:两台操作端同序号并发下载,源端必须都接受且互不
    /// 污染(命名空间隔离)。
    #[tokio::test]
    async fn accept_two_operators_same_counter() {
        let src = test_state("srcB");
        let dir = abs_dir();
        let (_keep, f) = make_file();
        let f_str = f.to_string_lossy().into_owned();

        src.jobs()
            .accept_orchestration(&src, "opA:1", "opA", std::slice::from_ref(&f_str), &dir, TransferPolicy::default())
            .await
            .unwrap();
        src.jobs()
            .accept_orchestration(&src, "opC:1", "opC", &[f_str], &dir, TransferPolicy::default())
            .await
            .unwrap();

        let snaps = src.jobs().snapshot();
        let ids: Vec<&str> = snaps.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"opA:1"), "操作端 A 的作业必须在场: {ids:?}");
        assert!(ids.contains(&"opC:1"), "操作端 C 的作业必须在场: {ids:?}");
        assert_eq!(ids.len(), 2, "同序号不同操作端不得互斥/覆盖: {ids:?}");
    }

    /// 评审 Critical#1a:纯数字外部 id 插入即前推自增计数器,后续 mint
    /// 永不与回显条目同号(否则注册表插入静默覆盖、驱动孤儿化)。
    #[tokio::test]
    async fn explicit_numeric_id_advances_counter() {
        let src = test_state("srcB");
        let dir = abs_dir();
        let (_keep, f) = make_file();
        let f_str = f.to_string_lossy().into_owned();

        src.jobs()
            .accept_orchestration(&src, "50", "opA", &[f_str], &dir, TransferPolicy::default())
            .await
            .unwrap();
        let minted = src
            .jobs()
            .start_upload(&src, vec![f], "peer", &dir, TransferPolicy::default(), false)
            .await
            .unwrap();

        assert_eq!(minted, "51", "自增面必须越过纯数字回显 id: {minted}");
        let snaps = src.jobs().snapshot();
        let ids: Vec<&str> = snaps.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"50"), "回显条目必须在场: {ids:?}");
        assert_eq!(ids.len(), 2, "不得覆盖: {ids:?}");
    }

    /// 评审 Important#2:源端 done 而本地轮已死(宽限内未收口)→ 作业必须
    /// 在有界窗口落 failed(此前 `running` 永久搁浅,wait_job 无界挂起)。
    #[tokio::test]
    async fn download_done_grace_fails_stranded_job() {
        let state = test_state("opA");
        let (core, _id) = make_download_card(&state, 1);
        let job = download_job();

        // 前两拍 running,第三拍起 done;宽限 120ms 内本地无收口 → failed
        let n = std::sync::atomic::AtomicU32::new(0);
        let mut poll = || {
            let k = n.fetch_add(1, Ordering::Relaxed);
            async move {
                if k < 2 {
                    Some(("running".to_string(), String::new()))
                } else {
                    Some(("done".to_string(), String::new()))
                }
            }
        };
        download_wait_terminal(
            &state,
            core.clone(),
            &job,
            Duration::from_millis(40),
            Duration::from_millis(120),
            SOURCE_POLL_FAIL_BOUND,
            &mut poll,
        )
        .await;

        let snap = core.snap();
        assert_eq!(snap.state.as_str(), "failed", "搁浅作业必须落 failed: {:?}", snap.state);
        assert!(
            snap.error.as_deref().unwrap_or_default().contains("源端已完成"),
            "error 必须说明源端已完成: {:?}",
            snap.error
        );
    }

    /// 评审 Important#2 反面:源端 done 但本地在宽限内自然收口 → done,
    /// 不得被过早判失败。
    #[tokio::test]
    async fn download_local_done_beats_source_done() {
        let state = test_state("opA");
        let (core, _id) = make_download_card(&state, 1);
        let job = download_job();

        let mut poll = || async { Some(("done".to_string(), String::new())) };
        let finisher = {
            let core = core.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(120)).await;
                core.finish("done", None);
            })
        };
        download_wait_terminal(
            &state,
            core.clone(),
            &job,
            Duration::from_millis(40),
            Duration::from_secs(30),
            SOURCE_POLL_FAIL_BOUND,
            &mut poll,
        )
        .await;
        finisher.await.unwrap();

        assert_eq!(core.snap().state.as_str(), "done", "宽限内自然收口必须保持 done");
    }

    /// remote_copy 轮询映射(Task 8):from 端 done **即时**映射本端 done
    /// (无 download 的宽限窗语义——X 无本机数据面,from 发完 = to 已收口)。
    #[tokio::test]
    async fn remote_copy_poll_maps_source_done_immediately() {
        let state = test_state("opX");
        let id = "opX:7".to_string();
        let meta = JobMeta {
            files_total: 0,
            bytes_total: 0,
            rel_sizes: HashMap::new(),
            src: "srcA:/x".into(),
            dst: "dstB:/d".into(),
        };
        let core = Arc::new(JobCore::new(id.clone(), "remote_copy", meta, state.events(), state.jobs().count.clone()));
        {
            let m = lock_ok(&core.m);
            core.emit_locked(&m);
        }
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        lock_ok(&state.jobs().map).insert(
            id.clone(),
            JobHandle {
                core: core.clone(),
                pause: PauseGate::new(),
                cancel_tx,
                task: Mutex::new(None),
                round: Mutex::new(None),
            },
        );
        let job = RemoteCopyJob {
            from: "srcA".into(),
            paths: vec!["/x".into()],
            to: "dstB".into(),
            dir: "/d".into(),
            policy: TransferPolicy::default(),
            cut: false,
        };

        // 前两拍 running,第三拍起 done → 首见即收口(不等第二拍)
        let n = std::sync::atomic::AtomicU32::new(0);
        let mut poll = || {
            let k = n.fetch_add(1, Ordering::Relaxed);
            async move {
                if k < 2 {
                    Some(("running".to_string(), String::new()))
                } else {
                    Some(("done".to_string(), String::new()))
                }
            }
        };
        remote_copy_wait_terminal(
            &state,
            core.clone(),
            &job,
            Duration::from_millis(30),
            SOURCE_POLL_FAIL_BOUND,
            &mut poll,
        )
        .await;

        let snap = core.snap();
        assert_eq!(snap.state.as_str(), "done", "from 端 done 必须即时映射: {:?}", snap.state);
        assert!(snap.error.is_none(), "成功作业无 error: {:?}", snap.error);
    }

    /// 查询持续不可达(评审 Important:None 原为无限重试——A 中途下线即永
    /// 久 running,只有用户取消能救):连续失败超过 `fail_bound` → 本端卡片
    /// 失败收口,文案可行动。download 侧。
    #[tokio::test]
    async fn download_persistent_poll_failure_fails_card() {
        let state = test_state("opA");
        let (core, _id) = make_download_card(&state, 1);
        let job = download_job();

        // 恒 None:t0 首败计时,t120(>100ms 界)失败收口
        let mut poll = || async { None::<(String, String)> };
        download_wait_terminal(
            &state,
            core.clone(),
            &job,
            Duration::from_millis(40),
            Duration::from_secs(30),
            Duration::from_millis(100),
            &mut poll,
        )
        .await;

        let snap = core.snap();
        assert_eq!(snap.state.as_str(), "failed", "持续不可达必须失败收口: {:?}", snap.state);
        assert!(
            snap.error.as_deref().unwrap_or_default().contains("持续不可达"),
            "error 必须可行动(持续不可达): {:?}",
            snap.error
        );
    }

    /// 单次瞬时失败不烧计数(归零生效):None → None → 成功查询归零 →
    /// None → None → done。时序设计(bound=100ms,tick=50ms):若归零失效,
    /// 第四拍(t150)累计 ≥100ms 已翻 failed;本测断言卡片收 done,且终态
    /// 由 finisher 给出(本地自然收口先于失败界)。
    #[tokio::test]
    async fn download_transient_poll_failure_resets_counter() {
        let state = test_state("opA");
        let (core, _id) = make_download_card(&state, 1);
        let job = download_job();

        let step = std::sync::atomic::AtomicUsize::new(0);
        let mut poll = || {
            let k = step.fetch_add(1, Ordering::Relaxed);
            async move {
                match k {
                    0 | 1 => None,
                    2 => Some(("running".to_string(), String::new())), // 归零点
                    3 | 4 => None,                                     // 新窗(重置已生效)
                    _ => Some(("running".to_string(), String::new())),
                }
            }
        };
        // 300ms 后本地自然收口(waiter 臂出;终态粘滞使与失败界竞速无害)
        let finisher = {
            let core = core.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                core.finish("done", None);
            })
        };
        download_wait_terminal(
            &state,
            core.clone(),
            &job,
            Duration::from_millis(50),
            Duration::from_secs(30),
            Duration::from_millis(100),
            &mut poll,
        )
        .await;
        finisher.await.unwrap();

        assert_eq!(
            core.snap().state.as_str(),
            "done",
            "瞬时失败(任一成功归零)不得翻车: {:?}",
            core.snap().state
        );
    }

    /// 查询持续不可达 → 失败收口(同 [`download_persistent_poll_failure_
    /// fails_card`],remote_copy 侧;与 download 共用同一兜底语义)。
    #[tokio::test]
    async fn remote_copy_persistent_poll_failure_fails_card() {
        let state = test_state("opX");
        let id = "opX:9".to_string();
        let meta = JobMeta {
            files_total: 0,
            bytes_total: 0,
            rel_sizes: HashMap::new(),
            src: "srcA:/x".into(),
            dst: "dstB:/d".into(),
        };
        let core = Arc::new(JobCore::new(id.clone(), "remote_copy", meta, state.events(), state.jobs().count.clone()));
        {
            let m = lock_ok(&core.m);
            core.emit_locked(&m);
        }
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        lock_ok(&state.jobs().map).insert(
            id.clone(),
            JobHandle {
                core: core.clone(),
                pause: PauseGate::new(),
                cancel_tx,
                task: Mutex::new(None),
                round: Mutex::new(None),
            },
        );
        let job = RemoteCopyJob {
            from: "srcA".into(),
            paths: vec!["/x".into()],
            to: "dstB".into(),
            dir: "/d".into(),
            policy: TransferPolicy::default(),
            cut: false,
        };

        // 恒 None:t120(>100ms 界)失败收口
        let mut poll = || async { None::<(String, String)> };
        remote_copy_wait_terminal(
            &state,
            core.clone(),
            &job,
            Duration::from_millis(40),
            Duration::from_millis(100),
            &mut poll,
        )
        .await;

        let snap = core.snap();
        assert_eq!(snap.state.as_str(), "failed", "持续不可达必须失败收口: {:?}", snap.state);
        assert!(
            snap.error.as_deref().unwrap_or_default().contains("持续不可达"),
            "error 必须可行动(持续不可达): {:?}",
            snap.error
        );
    }

    /// 单次瞬时失败不烧计数(remote_copy 侧,归零设计同
    /// [`download_transient_poll_failure_resets_counter`]):若归零失效,
    /// t150 累计 ≥100ms 已翻 failed;本测断言卡片收 done。
    #[tokio::test]
    async fn remote_copy_transient_poll_failure_resets_counter() {
        let state = test_state("opX");
        let id = "opX:11".to_string();
        let meta = JobMeta {
            files_total: 0,
            bytes_total: 0,
            rel_sizes: HashMap::new(),
            src: "srcA:/x".into(),
            dst: "dstB:/d".into(),
        };
        let core = Arc::new(JobCore::new(id.clone(), "remote_copy", meta, state.events(), state.jobs().count.clone()));
        {
            let m = lock_ok(&core.m);
            core.emit_locked(&m);
        }
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        lock_ok(&state.jobs().map).insert(
            id.clone(),
            JobHandle {
                core: core.clone(),
                pause: PauseGate::new(),
                cancel_tx,
                task: Mutex::new(None),
                round: Mutex::new(None),
            },
        );
        let job = RemoteCopyJob {
            from: "srcA".into(),
            paths: vec!["/x".into()],
            to: "dstB".into(),
            dir: "/d".into(),
            policy: TransferPolicy::default(),
            cut: false,
        };

        let step = std::sync::atomic::AtomicUsize::new(0);
        let mut poll = || {
            let k = step.fetch_add(1, Ordering::Relaxed);
            async move {
                match k {
                    0 | 1 => None,
                    2 => Some(("running".to_string(), String::new())), // 归零点
                    3 | 4 => None,                                     // 新窗(重置已生效)
                    _ => Some(("running".to_string(), String::new())),
                }
            }
        };
        // 300ms 后本地自然收口(cancel 竞速路径之外唯一出口;粘滞无害)
        let finisher = {
            let core = core.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                core.finish("done", None);
            })
        };
        remote_copy_wait_terminal(
            &state,
            core.clone(),
            &job,
            Duration::from_millis(50),
            Duration::from_millis(100),
            &mut poll,
        )
        .await;
        finisher.await.unwrap();

        assert_eq!(
            core.snap().state.as_str(),
            "done",
            "瞬时失败(任一成功归零)不得翻车: {:?}",
            core.snap().state
        );
    }

    /// M4a-1(终态保留上限):灌入 CAP+2 条终态作业后,注册表恰保 CAP 条
    /// ——最旧两条按 id seq 升序被驱逐,其余在场。
    #[tokio::test]
    async fn terminal_cap_evicts_oldest() {
        let state = test_state("opA");
        let dev = state.self_device_id().unwrap();
        for seq in 1..=(TERMINAL_RETAIN_CAP + 2) as u64 {
            make_terminal_card(&state, seq);
        }
        let snaps = state.jobs().snapshot();
        let ids: Vec<&str> = snaps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(snaps.len(), TERMINAL_RETAIN_CAP, "终态保留恰为 CAP: {ids:?}");
        assert!(!ids.contains(&format!("{dev}:1").as_str()), "最旧一条必须被驱逐: {ids:?}");
        assert!(!ids.contains(&format!("{dev}:2").as_str()), "次旧一条必须被驱逐: {ids:?}");
        assert!(ids.contains(&format!("{dev}:3").as_str()), "第三旧起必须在场: {ids:?}");
        assert!(
            ids.contains(&format!("{dev}:{}", TERMINAL_RETAIN_CAP + 2).as_str()),
            "最新一条必须在场: {ids:?}"
        );
    }

    /// M4a-1(驱逐后存在性):对已驱逐 id 调 `wait` 必须立即 Err(报文
    /// 「作业不存在: <id>」),绝不长轮询挂起——timeout(1s) 包裹,超时
    /// 即败。
    #[tokio::test]
    async fn wait_on_evicted_errors_fast() {
        let state = test_state("opA");
        let dev = state.self_device_id().unwrap();
        for seq in 1..=(TERMINAL_RETAIN_CAP + 2) as u64 {
            make_terminal_card(&state, seq);
        }
        let evicted = format!("{dev}:1");
        assert!(
            !state.jobs().snapshot().iter().any(|s| s.id == evicted),
            "前置:目标 id 必须已被驱逐"
        );
        let inner = tokio::time::timeout(Duration::from_secs(1), state.jobs().wait(&evicted))
            .await
            .expect("wait 必须立即返回(timeout = 长轮询悬挂)");
        let Err(err) = inner else {
            panic!("已驱逐 id 必须 Err,不得合成快照")
        };
        assert_eq!(err, format!("作业不存在: {evicted}"), "报文必须可辨: {err}");
    }

    /// M4a-2(毒锁恢复):持锁 panic 毒化 std Mutex 后,`lock_ok` 仍必须
    /// 取到守卫并可写(Err(poisoned) → into_inner)——锁面不再因一次毒化
    /// 对所有后续访问永久 panic。
    #[test]
    fn lock_ok_recovers_poison() {
        let m = Mutex::new(1u64);
        let _hook_guard = PANIC_HOOK_LOCK.lock().unwrap();
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // 预期 panic,静默输出
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // 原生 std 锁持守卫 panic → 毒化( arrange 不经被测函数)
            let _g = m.lock().unwrap();
            panic!("poison the lock");
        }))
        .is_err();
        std::panic::set_hook(prev);
        assert!(poisoned, "前置:锁必须已被毒化");
        let mut g = lock_ok(&m);
        *g += 1;
        assert_eq!(*g, 2, "毒锁恢复后必须可读写");
    }

    /// M4a-2(驱动监督):驱动 panic(以合成 panic future 代驱动本体)→
    /// `supervise_driver` 捕获 unwind 且作业未终态 → 落 failed(文案钉死
    /// 「内部错误: 传输驱动异常退出」);已终态再遇 panic 不得改写首次
    /// 终态(粘滞,不覆盖取消等先到终态)。
    #[tokio::test]
    // PANIC_HOOK_LOCK 守卫横跨两段 hook 换装间的 await:争用者仅另一条同步
    // #[test],且 #[tokio::test] 默认 current_thread 运行时,持 std 锁过
    // await 不会跨线程调度,无死锁面。
    #[allow(clippy::await_holding_lock)]
    async fn panicking_driver_marks_failed() {
        let state = test_state("opA");
        let meta = JobMeta {
            files_total: 0,
            bytes_total: 0,
            rel_sizes: HashMap::new(),
            src: "srcB:/x".into(),
            dst: "/dst".into(),
        };
        let core = Arc::new(JobCore::new(
            "sup:1".into(),
            "download",
            meta,
            state.events(),
            state.jobs().count.clone(),
        ));
        let _hook_guard = PANIC_HOOK_LOCK.lock().unwrap();
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // 预期 panic,静默输出
        supervise_driver(core.clone(), async { panic!("boom") }).await;
        std::panic::set_hook(prev);
        let snap = core.snap();
        assert_eq!(
            snap.state.as_str(),
            "failed",
            "panic 驱动必须落 failed: {:?}",
            snap.state
        );
        assert_eq!(
            snap.error.as_deref(),
            Some("内部错误: 传输驱动异常退出"),
            "文案钉死: {:?}",
            snap.error
        );
        // 已终态:再遇 panic 不改写(终态粘滞;监督不覆盖先到终态)
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        supervise_driver(core.clone(), async { panic!("again") }).await;
        std::panic::set_hook(prev);
        let snap = core.snap();
        assert_eq!(snap.state.as_str(), "failed", "终态粘滞: {:?}", snap.state);
        assert_eq!(
            snap.error.as_deref(),
            Some("内部错误: 传输驱动异常退出"),
            "首次终态文案不得被改写: {:?}",
            snap.error
        );
    }

    /// M4a-1(驱逐只瞄准终态):满员注册表再触发驱逐时,活跃作业必须
    /// 存活,被逐的是最旧终态。
    #[tokio::test]
    async fn terminal_cap_never_evicts_active() {
        let state = test_state("opA");
        let dev = state.self_device_id().unwrap();
        for seq in 1..=(TERMINAL_RETAIN_CAP + 1) as u64 {
            make_terminal_card(&state, seq);
        }
        // 满员(CAP+1 终态已把 seq 1 挤出)后再插入活跃卡 + 一条终态触发驱逐
        let (active_core, active_id) = make_download_card(&state, TERMINAL_RETAIN_CAP as u64 + 2);
        assert_eq!(active_core.snap().state, "queued", "活跃卡(非终态)入场");
        make_terminal_card(&state, TERMINAL_RETAIN_CAP as u64 + 3);
        let snaps = state.jobs().snapshot();
        assert!(snaps.iter().any(|s| s.id == active_id), "活跃作业永不驱逐");
        assert!(
            !snaps.iter().any(|s| s.id == format!("{dev}:2")),
            "被逐的必须是最旧终态(seq 2),不是活跃卡: {:?}",
            snaps.iter().map(|s| &s.id).collect::<Vec<_>>()
        );
        assert_eq!(
            snaps.len(),
            TERMINAL_RETAIN_CAP + 1,
            "CAP 条终态 + 1 条活跃: {}",
            snaps.len()
        );
    }
}
