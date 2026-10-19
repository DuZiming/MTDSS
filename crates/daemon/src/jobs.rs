//! 传输作业注册表 + upload/download/local_copy 驱动 + 速度 EMA + 事件节流
//! (M3c Task 6/7)。
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
//!   cancel/pause 直达绑定的轮,进度回调随轮进 RecvCtl。
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
//!   RecvProgress 逐步填 done/bytes。
//!
//! - **local_copy(Task 7)**:本机复制(spawn_blocking 跑 `fs::copy_paths`,
//!   cut=false)或移动快路(`fs::move_entries` 逐顶层条目 rename,cut=true;
//!   文件覆盖语义、目录撞名逐对失败)。纯本地操作不要求登录;fs 层无逐
//!   字节进度,作业卡以预扫计数呈现,完成时一次性补齐 done/bytes。
//!
//! - **匿名 incoming 卡(Task 7)**:ConnectRequest 不带 job_id(legacy
//!   send / 对端无任务面)→ resident loop 注册 kind="incoming" 的匿名卡
//!   并绑定轮——每笔传输都有可见卡片。轮失败即落 failed(源端重试会起新
//!   卡),轮成功落 done。

use crate::state::DaemonState;
use mtdss_core::transport::intake::TransferPolicy;
use mtdss_core::transport::transfer::{PauseGate, RecvProgress, SendProgress};
use mtdss_ipc::EventFrame;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
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

/// 是否终态(粘滞:落定后任何再翻转无操作)。
fn is_terminal_state(s: &str) -> bool {
    matches!(s, "done" | "failed" | "cancelled")
}

/// 策略上限校验(全局约束:合计 ≤2048 条、单条 ≤4096 字节);upload/
/// download 起作业共用同一同步反馈面。
fn check_policy_limits(policy: &TransferPolicy) -> Result<(), String> {
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

/// 传输作业注册表:id 单调分配(u64 计数,字符串对外),map 持全部作业
/// (终态不摘除——transfer.list 的历史面与 wait 的存在性都依赖它)。
pub(crate) struct Jobs {
    next_id: AtomicU64,
    map: Mutex<HashMap<String, JobHandle>>,
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
}

impl JobCore {
    /// 建队(queued):不在此发事件——调用方在建队原子段内发首帧。
    fn new(id: String, kind: &'static str, meta: JobMeta, events: tokio::sync::broadcast::Sender<EventFrame>) -> Self {
        Self {
            id,
            kind,
            meta,
            m: Mutex::new(JobMut {
                state: "queued".into(),
                done: HashSet::new(),
                file_bytes: HashMap::new(),
                samples: Vec::new(),
                started: Instant::now(),
                last_progress_emit: None,
                error: None,
            }),
            notify: Notify::new(),
            events,
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
                "error": snap.error,
            }),
        });
    }

    /// 状态翻转。终态粘滞(已终态 → 无操作);非终态同态翻转幂等无事件。
    /// 终态落定:置状态 + 事件即发 + notify_waiters(唤醒全部 wait 长轮询)。
    fn transition(&self, new_state: &str, error: Option<String>) {
        let mut m = self.m.lock().unwrap();
        if is_terminal_state(&m.state) {
            return; // 终态粘滞:cancel/完成竞速,先到者胜
        }
        if is_terminal_state(new_state) {
            m.state = new_state.to_string();
            m.error = error;
            self.emit_locked(&m);
            drop(m);
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
            let m = self.m.lock().unwrap();
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
        let mut m = self.m.lock().unwrap();
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
        let mut m = self.m.lock().unwrap();
        if is_terminal_state(&m.state) {
            return;
        }
        for (rel, size) in &self.meta.rel_sizes {
            m.done.insert(rel.clone());
            m.file_bytes.insert(rel.clone(), (*size, *size));
        }
    }

    /// 当前快照。
    fn snap(&self) -> JobSnap {
        let m = self.m.lock().unwrap();
        self.snap_locked(&m)
    }

    fn is_terminal(&self) -> bool {
        is_terminal_state(&self.m.lock().unwrap().state)
    }
}

impl Jobs {
    pub(crate) fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            map: Mutex::new(HashMap::new()),
        }
    }

    /// 起一个 upload 作业(自动 id;download 编排的回显入口走
    /// [`Jobs::accept_orchestration`] → [`Jobs::start_upload_internal`])。
    ///
    /// 校验(超限/非法即 Err → RPC code 1):files 非空;dir 合法(Task 1
    /// 绝对形态口径,不接受空 = 本 RPC 显式携带定向目录,与遗留 send 的
    /// 可缺省语义分开);策略上限(全局约束:合计 ≤2048 条、单条 ≤4096
    /// 字节)。预扫为空(输入缺失/全符号链接)同样 Err——作业化传输有
    /// 同步反馈面,不把注定失败的编排放进异步驱动。
    pub(crate) fn start_upload(
        &self,
        state: &Arc<DaemonState>,
        files: Vec<PathBuf>,
        to: &str,
        dir: &str,
        policy: TransferPolicy,
        cut_src: bool,
    ) -> Result<String, String> {
        self.start_upload_internal(state, None, files, to, dir, policy, cut_src)
    }

    /// start_upload 的内部本体:`explicit_id` 为 Some 时采用外部作业 id
    /// (download 编排的回显,Task 7),已存在即 Err(操作端重试换新 id,
    /// 不撞);None 走单调计数分配。
    #[allow(clippy::too_many_arguments)] // 内部本体吃下 start_upload 与编排回显两侧的参数面(回显侧多 explicit_id 一位)
    fn start_upload_internal(
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
        if !crate::flow::absolute_form(dir) {
            return Err(format!("dir 非法(须为绝对路径,Task 1 口径): {dir}"));
        }
        check_policy_limits(&policy)?;
        if state.session().is_none() {
            return Err("未登录,请先 login.verify".into());
        }
        // 预扫:全树 rel(只读元数据)——files/bytes 总数与 FileDone 补齐表
        let rels = crate::ops::local_tree_rels(&files);
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
                if self.map.lock().unwrap().contains_key(&id) {
                    return Err(format!("任务 id 已存在,拒绝编排回显: {id}"));
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
        ));
        // 建队原子段:首帧事件(queued)在插入注册表前发出——job_id 尚未
        // 返回给调用方,不存在「外部已见事件却查无此业」的窗口。
        {
            let m = core.m.lock().unwrap();
            core.emit_locked(&m);
        }
        // spawn 不会立即执行(start_upload 无 await):插入先于驱动首步,
        // transfer.cancel 不存在找不到条目的竞窗。
        let driver = tokio::spawn(upload_driver(
            state.clone(),
            core.clone(),
            UploadJob {
                files,
                to: to.to_string(),
                dir: dir.to_string(),
                policy,
                cut_src,
            },
            pause.clone(),
            cancel_rx,
        ));
        self.map.lock().unwrap().insert(
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
    /// 之携带,操作端 resident loop 据 id 绑定路由)。校验:job_id 非空
    /// (空 id 无法回绑)、to_device 非空、id 未被占用;其余校验(路径存
    /// 在性/目录形态/策略上限/登录)沿用 start_upload 的同步反馈面,失败
    /// 文案经 OrchestrateResponse.detail 回给操作端落 failed。
    pub(crate) fn accept_orchestration(
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
        )?;
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

        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
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
        ));
        {
            let m = core.m.lock().unwrap();
            core.emit_locked(&m);
        }
        let driver = tokio::spawn(download_driver(
            state.clone(),
            core.clone(),
            DownloadJob {
                src_device: device.to_string(),
                paths,
                dir: dir.to_string(),
                policy,
                cut,
            },
            pause.clone(),
        ));
        // 取消通知哨:独立任务等本地终态,cancelled 即通知源端停驱动重试
        // (abort 杀不到;done/failed 自然退出)。哨无 JoinHandle 追踪——
        // 作业终态即自灭,生命周期有界。
        let watchman_state = state.clone();
        let watchman_id = id.clone();
        let watchman_device = device.to_string();
        tokio::spawn(async move {
            let fin = watchman_state.jobs().wait(&watchman_id).await;
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
        self.map.lock().unwrap().insert(
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

    /// 起一个 local_copy 作业(Task 7):本机复制/移动(spawn_blocking 跑
    /// core fs)。纯本地操作**不要求登录**;校验:paths 非空、dir 存在且为
    /// 目录、dir 合法(绝对形态)、预扫非空(与 upload 同口径的同步反馈面)。
    pub(crate) fn start_local_copy(
        &self,
        state: &Arc<DaemonState>,
        paths: Vec<PathBuf>,
        dir: &str,
        cut: bool,
    ) -> Result<String, String> {
        if paths.is_empty() {
            return Err("paths 不能为空".into());
        }
        if !crate::flow::absolute_form(dir) {
            return Err(format!("dir 非法(须为绝对路径,Task 1 口径): {dir}"));
        }
        if !std::path::Path::new(dir).is_dir() {
            return Err(format!("目标目录不存在或不是目录: {dir}"));
        }
        // 预扫:全树 rel(只读元数据)——files/bytes 总数与完成补齐表
        let rels = crate::ops::local_tree_rels(&paths);
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
        ));
        {
            let m = core.m.lock().unwrap();
            core.emit_locked(&m);
        }
        let driver = tokio::spawn(
            local_copy_driver(
                core.clone(),
                LocalCopyJob {
                    paths,
                    dir: dir.to_string(),
                    cut,
                },
                pause.clone(),
            ),
        );
        self.map.lock().unwrap().insert(
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
        ));
        {
            let m = core.m.lock().unwrap();
            core.emit_locked(&m);
        }
        // 无驱动任务:轮的收口(resident loop)即作业收口;cancel/暂停经
        // round 槽位直达该轮。
        self.map.lock().unwrap().insert(
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
        let core = self.map.lock().unwrap().get(job_id).map(|h| h.core.clone());
        if let Some(core) = core {
            core.track_recv(p);
        }
    }

    /// 轮终局收口(Task 7,常驻接收循环调用):绑定下载与匿名卡共用——
    /// done/failed 经 transition 落定(终态粘滞使与 cancel 的竞速无害)。
    /// 作业不存在 → false(调用方忽略)。
    pub(crate) fn finish_incoming(&self, job_id: &str, state: &str, error: Option<String>) -> bool {
        let core = self.map.lock().unwrap().get(job_id).map(|h| h.core.clone());
        match core {
            Some(core) => {
                core.finish(state, error);
                true
            }
            None => false,
        }
    }

    /// 全量快照(transfer.list 数据源),按 id 数值升序。
    pub(crate) fn snapshot(&self) -> Vec<JobSnap> {
        let map = self.map.lock().unwrap();
        let mut out: Vec<(u64, JobSnap)> = map
            .iter()
            .map(|(id, h)| (id.parse::<u64>().unwrap_or(u64::MAX), h.core.snap()))
            .collect();
        out.sort_by_key(|(n, _)| *n);
        out.into_iter().map(|(_, s)| s).collect()
    }

    /// 暂停/恢复(false = resume):置任务暂停门 + 可见状态翻转;直达绑定的
    /// 接收轮门(Task 7 download 侧)。缺失或已终态 → false(不可暂停)。
    pub(crate) fn set_paused(&self, id: &str, paused: bool) -> bool {
        let core = {
            let map = self.map.lock().unwrap();
            let Some(h) = map.get(id) else {
                return false;
            };
            if h.core.is_terminal() {
                return false;
            }
            h.pause.set(paused);
            if let Some(r) = h.round.lock().unwrap().as_ref() {
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
            let map = self.map.lock().unwrap();
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
            if let Some(r) = h.round.lock().unwrap().as_ref() {
                let _ = r.cancel.send(true);
            }
            h.core.transition("cancelled", None);
            task = h.task.lock().unwrap().take();
        }
        if let Some(task) = task {
            task.abort(); // 连 run_send_with 重试循环一并杀,不复活
        }
        true
    }

    /// 终态长轮询(notify 唤醒)。api 层已保证 id 存在(作业不摘除,存在性
    /// 稳定);防御分支(不可达)合成 failed 快照,绝不悬挂。
    pub(crate) async fn wait(&self, id: &str) -> JobSnap {
        let Some(core) = self.map.lock().unwrap().get(id).map(|h| h.core.clone()) else {
            return JobSnap {
                id: id.to_string(),
                kind: "upload",
                state: "failed".into(),
                files_done: 0,
                files_total: 0,
                bytes_done: 0,
                bytes_total: 0,
                speed_bps: 0,
                src: String::new(),
                dst: String::new(),
                error: Some("任务不存在".into()),
            };
        };
        loop {
            // enable 先于查终态:Notified 的登记序闭合「翻转与登记竞速」的
            // 漏唤醒窗口(见 tokio Notify 文档的循环用法)
            let notified = core.notify.notified();
            tokio::pin!(notified);
            let _ = notified.as_mut().enable();
            let snap = core.snap();
            if is_terminal_state(&snap.state) {
                return snap;
            }
            notified.await;
        }
    }

    /// resident 接收轮绑定(Task 7 接线):job_id 存在**且为可绑定的来件
    /// 作业**(download = 操作端预注册的下载卡;incoming = 本机刚注册的
    /// 匿名卡)即占槽,返回是否绑定成功。普通 upload 永不作为绑定目标——
    /// 跨设备普通作业的 id 撞号(两台各自从 1 起数)不构成绑定意图,误
    /// 路由会把远端来件的进度灌进本地卡。重复绑定后写者胜(下载轮代际
    /// 更替重绑的自然覆盖)。
    pub(crate) fn bind_incoming(&self, job_id: &str, round: RoundHandle) -> bool {
        let map = self.map.lock().unwrap();
        match map.get(job_id) {
            Some(h) if matches!(h.core.kind, "download" | "incoming") => {
                *h.round.lock().unwrap() = Some(round);
                true
            }
            _ => false,
        }
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
/// 保持 running 等重来的 ConnectRequest;终局失败(源端重试耗尽/协议终局)
/// 由本探测收口。3s = 失败感知延迟与 ctrl 请求开销的折中(池内连接复用,
/// 单次请求毫秒级)。
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
    // 终局等待:本地终态(轮成功/取消)即出;轮询探测源端终局失败
    let mut ticker = tokio::time::interval(DOWNLOAD_SOURCE_POLL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let waiter = state.jobs().wait(&core.id);
    tokio::pin!(waiter);
    loop {
        tokio::select! {
            fin = &mut waiter => {
                if fin.state == "done" && job.cut {
                    cut_finalize_remote(&state, &job.src_device, &job.paths).await;
                }
                return;
            }
            _ = ticker.tick() => {
                match ctrl.job_query(&state, &job.src_device, &core.id).await {
                    Ok(q) if q.state == "failed" => {
                        let detail = if q.error.is_empty() {
                            "源端作业失败".to_string()
                        } else {
                            format!("源端作业失败: {}", q.error)
                        };
                        core.finish("failed", Some(detail));
                        return;
                    }
                    Ok(q) if q.state == "cancelled" => {
                        core.finish("cancelled", None);
                        return;
                    }
                    // missing:源端无此作业(重启/登出拆卸)——永不完成,按失败收口
                    Ok(q) if q.state == "missing" => {
                        core.finish("failed", Some("源端已无此作业(设备可能重启)".into()));
                        return;
                    }
                    _ => {}
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

// ---------- local_copy 驱动(Task 7) ----------

/// local_copy 驱动任务的载荷(spawn 边界一次性搬移)。
struct LocalCopyJob {
    /// 本机源路径(顶层)。
    paths: Vec<PathBuf>,
    /// 本机目标目录(已校验存在且为目录)。
    dir: String,
    /// true = 移动快路(逐顶层条目 rename);false = fs::copy_paths 复制。
    cut: bool,
}

/// local_copy 驱动:spawn_blocking 跑 core fs(整树复制/回收删除级阻塞调
/// 用直跑会占死异步 worker,与 ctrl 面同一收口纪律);起步先过暂停门(门
/// 上等恢复,cancel 由注册表 abort 本任务兜底——阻塞中的 fs 操作不可中断,
/// abort 落在 await 点,复制线程自然跑完被弃)。终局:成功 mark_all_done
/// 补齐计数面再 done;move 逐对失败表非空 → failed 携带明细。
async fn local_copy_driver(core: Arc<JobCore>, job: LocalCopyJob, pause: Arc<PauseGate>) {
    core.begin_running(pause.paused());
    if pause.paused() {
        pause.wait().await;
    }
    let paths = job.paths.clone();
    let dir = job.dir.clone();
    let cut = job.cut;
    let outcome = tokio::task::spawn_blocking(move || -> Result<(usize, Vec<mtdss_core::fs::PathOpFailure>), mtdss_core::fs::FsError> {
        if cut {
            // 移动快路:逐顶层条目 rename 入目标目录(文件覆盖既有文件、
            // 目录撞既有名逐对失败,与 rename 原生语义一致;跨卷回退暂存
            // 复制 + 源永久删除,fs 层透明处理)
            let mut pairs: Vec<(String, String)> = Vec::with_capacity(paths.len());
            for p in &paths {
                let Some(name) = p.file_name() else {
                    return Err(mtdss_core::fs::FsError::InvalidPath(format!(
                        "移动输入无文件名(卷根等): {}",
                        p.display()
                    )));
                };
                let to = std::path::Path::new(&dir).join(name);
                pairs.push((p.to_string_lossy().into_owned(), to.to_string_lossy().into_owned()));
            }
            mtdss_core::fs::move_entries(&pairs)
        } else {
            let srcs: Vec<String> = paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            mtdss_core::fs::copy_paths(&srcs, &dir).map(|landed| (landed.len(), Vec::new()))
        }
    })
    .await;
    match outcome {
        Ok(Ok((_n, failed))) if failed.is_empty() => {
            core.mark_all_done();
            core.finish("done", None);
        }
        Ok(Ok((moved, failed))) => {
            let detail: Vec<String> = failed
                .iter()
                .map(|f| format!("{}({}: {})", f.path, f.code, f.message))
                .collect();
            core.finish(
                "failed",
                Some(format!("移动 {moved} 项成功,{} 项失败: {}", failed.len(), detail.join("; "))),
            );
        }
        Ok(Err(e)) => core.finish("failed", Some(e.to_string())),
        Err(e) => core.finish("failed", Some(format!("本地复制任务收尾失败: {e}"))),
    }
}
