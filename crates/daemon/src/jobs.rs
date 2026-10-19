//! 传输作业注册表 + upload 驱动 + 速度 EMA + 事件节流(M3c Task 6)。
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
//!   (进度入注册表 + 作业侧取消/暂停直达该轮)。本任务只定义类型与注册
//!   表槽位:cancel/pause 已直达绑定的轮;resident loop 的实际接线在
//!   Task 7(download 侧)。

use crate::state::DaemonState;
use mtdss_core::transport::intake::TransferPolicy;
use mtdss_core::transport::transfer::{PauseGate, RecvProgress, SendProgress};
use mtdss_ipc::EventFrame;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{watch, Notify};
use tokio::task::JoinHandle;

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
    /// 进度回调(RecvProgress → 注册表):Task 7 在 resident loop 装配,
    /// Task 6 无读取点。
    #[allow(dead_code)] // Task 7 resident loop 接线前进度回调无读取点
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
        let mut m = self.m.lock().unwrap();
        if is_terminal_state(&m.state) {
            return; // 驱动收尾/取消后迟到的回调:丢弃
        }
        match p {
            SendProgress::FileDone { rel } => {
                m.done.insert(rel.to_string());
                // FileDone 不带总量:用预扫 size 补齐(skip 文件无 Bytes 回调,
                // 否则 bytes_done 永远缺它一份)
                let total = m
                    .file_bytes
                    .get(*rel)
                    .map(|(_, t)| *t)
                    .or_else(|| self.meta.rel_sizes.get(*rel).copied())
                    .unwrap_or(0);
                m.file_bytes.insert(rel.to_string(), (total, total));
            }
            SendProgress::Bytes { rel, done, total } => {
                let e = m.file_bytes.entry(rel.to_string()).or_insert((0, *total));
                if *done > e.0 {
                    e.0 = *done;
                }
                e.1 = *total;
            }
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

    /// 起一个 upload 作业:校验 → 预扫(local_tree_rels:计数/字节/rel 表)
    /// → 建队(queued + 首帧事件)→ spawn 驱动任务跑 run_send_with(带
    /// SendSpec{dir,job_id,policy} + ctl)。返回 job_id。
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
        if files.is_empty() {
            return Err("files 不能为空".into());
        }
        if !crate::flow::absolute_form(dir) {
            return Err(format!("dir 非法(须为绝对路径,Task 1 口径): {dir}"));
        }
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

        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
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

    /// resident 接收轮绑定(Task 7 接线):job_id 存在即占槽,返回是否
    /// 绑定成功。重复绑定后写者胜(轮代际更替时的自然覆盖)。
    /// Task 6 无调用方(类型与槽位先行),Task 7 的 resident loop 接线后
    /// 移除 allow。
    #[allow(dead_code)] // Task 7 resident loop 接线前无调用方(brief 钉死先行定义)
    pub(crate) fn bind_incoming(&self, job_id: &str, round: RoundHandle) -> bool {
        let map = self.map.lock().unwrap();
        match map.get(job_id) {
            Some(h) => {
                *h.round.lock().unwrap() = Some(round);
                true
            }
            None => false,
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
