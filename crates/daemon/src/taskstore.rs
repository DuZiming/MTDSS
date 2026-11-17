//! tasks.json:传输任务记录持久层(v2-P2 Task 3;v3p6 §5.1-4 扩面)。
//!
//! 传输任务的历史记录(卡片字段 + 重发原料)落 `{home}/tasks.json`,
//! 每次 [`TaskStore::upsert`]/[`TaskStore::remove`] 即时原子写(复用
//! [`crate::session::write_atomic`]:同目录临时件写全 fsync 后 rename,
//! 半写不可见;状态翻转即时落盘,窗口内崩溃最多回退为 interrupted,
//! 无损,spec §9)。文件缺失 = 首次,从空表开始;解析失败(外力损坏
//! /手改事故)与非 UTF-8 内容(read_to_string 的 InvalidData,同一损坏
//! 面)→ 改名 `tasks.json.bad-<ts>` 留底后从空开始 + `tracing::
//! warn`——任务记录是可丢的面板数据,拦启动不成比例(spec §9 定的
//! 「备份后从空」,与 settings「损坏即硬错误」口径不同)。字段全量
//! serde default:旧版/残缺记录前向兼容加载,缺字段取类型零值(如
//! name = ""、finished_ms = None)。
//!
//! **写失败活力面**(v3p6 Task 24 现场票:MIX3 tasks.json mtime 冻结排查
//! 收口):落盘失败不拦内存权威面,也**不存在停摆闩**——每次
//! upsert/remove 都原样再试(无后台驱动、无退避状态机,flush 内联于写
//! 调用),失败解除后的下一次成功写 = 全量补写,镜像自动愈合(中断
//! 期内存面新增的记录一并落盘)。可见性:warn 限频——仅在失败状态翻转
//! 时记一条(健康→失败首现、失败中错误文案变化),恢复时 info 一条;
//! 持续失败期间逐写静默(不刷屏),错误面经 `TaskStore::flush_failed`
//! (仅测试构建)可查。固有的边界:中断期内进程死亡,中断期新增记录只存内存面,重启
//! load 以盘上旧镜像为准(记录=可丢面板数据口径,不再加时钟重写/日志
//! 卷——面板数据不值,且后台定时重写违背「不空转」界)。
//!
//! 落盘形态(v3p6 §5.1-4 起):包装对象 `{next_incoming_id, recs}`——
//! 旧版裸数组仍兼容加载([`TaskStore::load`] 双形态解析,写恒为包装)。
//! `next_incoming_id` 是匿名 incoming 卡裸数字 id 域的**计数水位**:
//! incoming id 是进程级自增(重启归零即撞号),水位随写持久,jobs 的
//! 自增计数器以它播种(重启后新卡 id 越过全部已持久 id,计划 Review
//! Focus 4);已被上限退役/清除的 id 事实由字段留住,计数域不回退。
//!
//! **终态持久上限 100**(spec §5.1-4「终态持久上限 100」):终态记录
//! (done/failed/cancelled/interrupted 同池,含 incoming)超限即按终态
//! 时刻退役最老的([`TaskStore::retire_locked`],写路径收口,非终态永不
//! 退役);历史面有界,transfer.list 不随年月无界膨胀。
//!
//! 内存面 `Mutex<HashMap>` 为权威,文件是镜像;`load` 只在启动调一次,
//! 并把非终态(running/paused/queued/publishing)一律标记 interrupted——daemon
//! 死了传输必然断了,重启后置「中断」等用户重新发起(终态 done/
//! failed/cancelled 照旧)。持久层不含任何驱动/注册表逻辑:jobs 的
//! 活跃面(Task 4+)经 upsert/get/list 镜像进来,两套数据不互侵入。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::jobs::{job_seq, lock_ok};
use crate::session::write_atomic;

/// home 下任务记录文件名。
const TASKS_FILE: &str = "tasks.json";

/// 落盘函数注入缝(Task 24 测试面):生产 = [`write_atomic`] 适配;
/// 测试注入可失败/可观测的替身,钉「写失败→内存权威→恢复→全量补写」
/// 的活力性质(现场票缺的就是这枚锚)。
type FlushFn = Arc<dyn Fn(&Path, &str) -> std::io::Result<()> + Send + Sync>;

/// 生产落盘:write_atomic(anyhow::Result)适配成 io::Result。
fn atomic_writer() -> FlushFn {
    Arc::new(|path, body| {
        write_atomic(path, body).map_err(|e| std::io::Error::other(e.to_string()))
    })
}

/// 失败限频记账(纯函数,单测钉死):返回 true = 失败状态翻转(健康→
/// 失败首现,或失败中错误文案变化),调用方应记一条 warn;false = 同一
/// 错误持续,静默(不逐写刷日志)。恢复由调用方清空 `None` 并记 info。
fn note_failure(failed: &mut Option<String>, msg: &str) -> bool {
    let changed = match failed {
        None => true,
        Some(prev) => prev != msg,
    };
    if changed {
        *failed = Some(msg.to_string());
    }
    changed
}

/// 终态记录持久上限(spec §5.1-4「终态持久上限 100」):写路径退役面,
/// 见 [`TaskStore::retire_locked`]。
const TERMINAL_RECORD_CAP: usize = 100;

/// tasks.json 落盘形态(v3p6 §5.1-4 起包装对象;旧版裸数组仍兼容加载,
/// 见 [`TaskStore::load`])。两字段 serde default:残缺包装不拒载。
#[derive(Serialize, Deserialize, Default)]
struct StoreDump {
    /// 匿名 incoming(裸数字 id 域)水位:jobs 自增计数器的播种面,下一
    /// 张卡 id 必须 > 此值(v2-P2 的记录式推导保留为折叠兜底,见
    /// [`TaskStore::incoming_watermark`])。
    #[serde(default)]
    next_incoming_id: u64,
    /// 记录全量(id 升序)。
    #[serde(default)]
    recs: Vec<TaskRec>,
}

/// 单条传输任务记录(brief 字段面钉死,Task 4/6/7/8 消费)。全字段
/// serde default:旧/残记录缺字段即取类型零值,前向兼容加载。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)] // 全字段缺省(容器级:等价于逐字段 #[serde(default)])
pub(crate) struct TaskRec {
    /// 作业 id(`<dev>:<seq>` 命名空间;重发翻新同记录换新 job_id)。
    pub id: String,
    /// 作业类型(upload|download|remote_copy)。
    pub kind: String,
    /// 任务代表名(V3-1 §6:单文件本名/单目录顶层名/多项「首项名 等
    /// N 项」;发起端建卡算好,并经 ConnectRequest/OrchestrateRequest
    /// display_name 下发给接收端会话行/回显卡——两端同名)。
    pub name: String,
    /// 方向两端(upload 源端 = 本机)。
    pub from_dev: String,
    /// 方向另一端。
    pub to_dev: String,
    /// 源路径列表(upload = 本机路径;download = 源端设备上的路径;
    /// remote_copy = from 端路径)——重发的原料。
    pub src_paths: Vec<String>,
    /// 目标目录(接收设备视角)。
    pub target_dir: String,
    /// 冲突策略(skip 表)。
    pub policy_skip: Vec<String>,
    /// 冲突策略(overwrite 表)。
    pub policy_overwrite: Vec<String>,
    /// 剪切语义(成功后删源)。
    pub cut: bool,
    /// 预扫 rel 全集(重发 diff「消失的文件」用;job_query 回填前为空)。
    pub rels: Vec<String>,
    /// 进度:文件计数(中断面显示最后已知值)。
    pub files_done: u64,
    /// 进度:文件总数。
    pub files_total: u64,
    /// 进度:字节计数。
    pub bytes_done: u64,
    /// 进度:字节总数。
    pub bytes_total: u64,
    /// 状态(queued/running/paused/publishing/done/failed/cancelled/
    /// interrupted;publishing = P4a 收件发布窗「保存中」,非终态,重启
    /// 沿既有语义映射 interrupted)。
    pub state: String,
    /// 发送端作业 id(spec §7-3;v3p6 Task 11 续传进度种子):incoming
    /// 记录 ConnectRequest 携带的发送端作业 id,续传轮重启后按同键找到
    /// 前代卡的中断断面做进度种子([`TaskStore::incoming_progress_seed`]);
    /// 发起端记录(upload/download/remote_copy)恒空(download/remote_copy
    /// 的同键匹配走 rec.id——ConnectRequest 携带的正是操作端作业 id 自身)。
    pub src_job: String,
    /// 终态失败文案(失败原因;其余 None)。
    pub error: Option<String>,
    /// 创建时刻(epoch 毫秒)。
    pub created_ms: u64,
    /// 终态落定时刻(epoch 毫秒;未终态 None)。
    pub finished_ms: Option<u64>,
}

/// 任务记录持久层:内存 HashMap 为权威,`{home}/tasks.json` 为镜像。
/// 每次写操作(插入/覆盖/删除)同步落盘——记录写频极低(建队/翻转/
/// 进度节流回写),毫秒级临时件 rename 不构成热点;落盘持 map 锁串行,
/// 并发写不复现状半写临时件(见 `flush_locked`)。
pub(crate) struct TaskStore {
    /// id → 记录(进程内权威面)。
    map: Mutex<HashMap<String, TaskRec>>,
    /// 匿名 incoming 裸数字 id 域水位(v3p6 §5.1-4,见
    /// [`TaskStore::incoming_watermark`]):load 折叠「持久字段 ∪ 记录
    /// 推导」,upsert 随写前推(fetch_max,单调不回退)。与 map 同一写
    /// 临界区(upsert 持 map 锁全程含 flush),镜像值与记录集一致。
    incoming_wm: AtomicU64,
    /// 数据目录(落盘 `{home}/tasks.json`)。
    home: PathBuf,
    /// 落盘函数(生产 write_atomic;测试注入缝,见 [`FlushFn`])。
    writer: FlushFn,
    /// 落盘失败状态面(活力日志,见模块注释「写失败活力面」):None =
    /// 自上次成功以来无失败;Some = 持续失败中的最近错误文案。只在
    /// flush_locked 内取用(恒持 map 锁),取 [`lock_ok`] 沿模块惯例。
    failed: Mutex<Option<String>>,
}

/// 记录面水位推导:`max(job_seq(id)) + 1`(空表 0)——裸数字与命名空间
/// 两型 id 的序号同池取最大,与 jobs 自增面的播种口径一致;畸形 id 的
/// 哨兵值(`job_seq` 的 u64::MAX)不进水位,不毒化计数域。
fn derive_watermark(recs: &[TaskRec]) -> u64 {
    recs.iter()
        .map(|r| job_seq(&r.id))
        .filter(|s| *s != u64::MAX)
        .max()
        .map_or(0, |s| s.saturating_add(1))
}

/// store 视角的终态面(上限退役计数池):done/failed/cancelled 之外含
/// interrupted——它是 load 的重启恢复标记,记录已死(无驱动复活口),
/// 与其余终态同属「历史面」,一并计入上限;非终态在途记录有消费者,
/// 永不退役。
fn rec_retirable(state: &str) -> bool {
    matches!(state, "done" | "failed" | "cancelled" | "interrupted")
}

impl TaskStore {
    /// 启动装配:读 `{home}/tasks.json` 入内存,并把非终态标 interrupted
    /// (重启恢复,见模块注释)。文件缺失/读取失败/解析失败一律从空表
    /// 继续(解析失败与非 UTF-8 内容先备份 `.bad-<ts>` 留底);不返回
    /// Result——记录是可丢面板数据,任何异常都不拦 daemon 启动。
    ///
    /// 双形态解析(v3p6 §5.1-4):旧裸数组优先(存量文件 + 测试直写面),
    /// 失败再试包装对象;两形态皆败 = 损坏 → 备份从空开始。水位折叠
    /// 「持久字段 ∪ 记录推导」取大:字段是权威面(留住已退役/清除的 id
    /// 事实),推导兜住旧文件(无字段)与字段落后于记录的手改/外来文件。
    pub(crate) fn load(home: &Path) -> Self {
        Self::build(home, atomic_writer())
    }

    /// 装配本体(测试注入缝:[`TaskStore::load`] 的可换落盘函数形态)。
    fn build(home: &Path, writer: FlushFn) -> Self {
        let path = home.join(TASKS_FILE);
        let mut map = HashMap::new();
        let mut wm = 0u64;
        match std::fs::read_to_string(&path) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {} // 首次:无文件即空
            // 非 UTF-8 内容(InvalidData)= 外力损坏,与解析失败同一兜底
            // 面:备份后从空开始(spec §9)。静默按空表继续只留给真正的读
            // 取失败(权限/设备错误)——内容还在原位,不动它。
            Err(err) if err.kind() == std::io::ErrorKind::InvalidData => {
                backup_corrupt(home, &path, "非 UTF-8 内容", &err);
            }
            Err(err) => {
                tracing::warn!(%err, path = %path.display(), "tasks.json 读取失败,按空表继续")
            }
            Ok(s) => match serde_json::from_str::<Vec<TaskRec>>(&s) {
                // 旧裸数组:水位自记录推导(字段缺席)
                Ok(recs) => {
                    wm = derive_watermark(&recs);
                    mark_interrupted(recs, &mut map);
                }
                Err(err) => match serde_json::from_str::<StoreDump>(&s) {
                    Ok(dump) => {
                        wm = dump.next_incoming_id.max(derive_watermark(&dump.recs));
                        mark_interrupted(dump.recs, &mut map);
                    }
                    // 两形态皆败 = 损坏:备份后从空开始
                    Err(_) => backup_corrupt(home, &path, "解析失败", &err),
                },
            },
        }
        Self {
            map: Mutex::new(map),
            incoming_wm: AtomicU64::new(wm),
            home: home.to_path_buf(),
            writer,
            failed: Mutex::new(None),
        }
    }

    /// 落盘失败状态面(仅测试构建消费:活力性质的断言缝)。
    #[cfg(test)]
    fn flush_failed(&self) -> Option<String> {
        lock_ok(&self.failed).clone()
    }

    /// 测试装配:注入可失败落盘函数(见 [`FlushFn`])。
    #[cfg(test)]
    fn load_with_writer(home: &Path, writer: FlushFn) -> Self {
        Self::build(home, writer)
    }

    /// 匿名卡裸数字 id 域水位(v3p6 §5.1-4,计划 Review Focus 4):下一
    /// 张 incoming 卡 id 必须 > 此值。jobs 自增计数器以它播种(重启不复
    /// 用已持久 id);upsert 随写前推,单调不回退(退役/清除不降)。
    pub(crate) fn incoming_watermark(&self) -> u64 {
        self.incoming_wm.load(Ordering::Relaxed)
    }

    /// 插入/覆盖一条记录并即时落盘(状态翻转即持久,spec §9)。落盘持
    /// 同一把 map 锁全程(见 [`TaskStore::flush_locked`]):不同线程的并发
    /// 写(worker 进度回写 / RPC 状态翻转)在锁上串行,不复现两写者交错
    /// 写坏 `tasks.json.tmp` 的撕裂窗。插入同时:①水位按记录 id 前推
    /// (incoming 与 upload/local_copy 共用同一自增域,记录 id 是水位的
    /// 权威事实面;畸形 id 不前推);②终态超限退役(见
    /// [`TaskStore::retire_locked`])——两件事并入同一次落盘,不留中间态
    /// 镜像。
    pub(crate) fn upsert(&self, rec: &TaskRec) {
        let seq = job_seq(&rec.id);
        if seq != u64::MAX {
            self.incoming_wm
                .fetch_max(seq.saturating_add(1), Ordering::Relaxed);
        }
        let mut map = lock_ok(&self.map);
        map.insert(rec.id.clone(), rec.clone());
        self.retire_locked(&mut map);
        self.flush_locked(&map);
    }

    /// 按 id 删除记录并即时落盘。命中 true;不存在 false(不空转落盘)。
    /// 消费者:reinitiate 的翻新摘旧(v2-P2 Task 6)与 Jobs::clear
    /// (transfer.clear,Task 7)。水位不随删除回退(单调面:已铸造的
    /// id 事实不因记录摘除而失真)。
    pub(crate) fn remove(&self, id: &str) -> bool {
        let mut map = lock_ok(&self.map);
        let removed = map.remove(id).is_some();
        if removed {
            self.flush_locked(&map);
        }
        removed
    }

    /// 按 id 取一条(克隆快照,调用方可自由改写后再 upsert 回)。
    pub(crate) fn get(&self, id: &str) -> Option<TaskRec> {
        lock_ok(&self.map).get(id).cloned()
    }

    /// 续传进度种子查前代(v3p6 Task 11,spec §7-4「续传进度语义」):同
    /// (来源设备, 发送端作业 id) 的**最近一张**未完成中断卡(failed/
    /// interrupted)的中断断面进度 `(bytes_done, files_done)`——重试轮新建
    /// 匿名卡以它为进度显示面下限,「跨轮累计」观感在前代卡已终态的形态
    /// 下不断档(进程内前代卡另有逐文件精确移植,见 jobs 侧;本查走 store
    /// 同一覆盖两态:进程内 failed 卡与重启 interrupted 记录)。
    /// 匹配两形:incoming 记录按 `rec.src_job`(建卡点写入,spec §7-3);
    /// download/remote_copy 记录按 `rec.id`(ConnectRequest 携带的就是操作
    /// 端作业 id 自身,重试轮匿名重绑路径同键)。done/cancelled 不出种子
    /// (完成已收口 = 重试是新传输;用户取消 = 止步意图——盘上引擎位图
    /// 续传不受此查影响,只管卡片显示面);断面双零(未起字节数即死的卡)
    /// 同视为无种子。空 src_job(legacy 来件)恒 None。线性扫(会话级记
    /// 录量小,仅 incoming 建卡路径一扫,毫秒级);多命中取 created_ms 最
    /// 新(平局按 id 序号,钉死确定性——种子面恒取最近断面)。
    pub(crate) fn incoming_progress_seed(
        &self,
        from_dev: &str,
        src_job: &str,
    ) -> Option<(u64, u64)> {
        if src_job.is_empty() {
            return None;
        }
        let map = lock_ok(&self.map);
        map.values()
            .filter(|r| {
                r.from_dev == from_dev
                    && matches!(r.state.as_str(), "failed" | "interrupted")
                    && (r.bytes_done > 0 || r.files_done > 0)
                    && ((r.kind == "incoming" && r.src_job == src_job)
                        || (matches!(r.kind.as_str(), "download" | "remote_copy")
                            && r.id == src_job))
            })
            .max_by_key(|r| (r.created_ms, job_seq(&r.id)))
            .map(|r| (r.bytes_done, r.files_done))
    }

    /// 全量快照,id 字典序升序(`<dev>:<seq>` 字符串序)。
    pub(crate) fn list(&self) -> Vec<TaskRec> {
        let mut recs: Vec<TaskRec> = lock_ok(&self.map).values().cloned().collect();
        recs.sort_by(|a, b| a.id.cmp(&b.id));
        recs
    }

    /// 终态记录上限退役(spec §5.1-4「终态持久上限 100」):终态总数超
    /// 限即按终态时刻退役最老的,直到 ≤ 上限。done/failed/cancelled/
    /// interrupted 同池计数(**含 incoming**;interrupted = 重启恢复标记,
    /// 记录已死无复活口);非终态(queued/running/paused/publishing)
    /// 永不退役。终态时刻取 finished_ms,缺席回退 created_ms,id 兜底保
    /// 证全序确定。**持 map 锁调用**:退役并入本次落盘(一次原子写同时
    /// 收插入与退役,镜像不留「插了未退」的中间态)。
    fn retire_locked(&self, map: &mut HashMap<String, TaskRec>) {
        let mut terminals: Vec<(Option<u64>, u64, String)> = map
            .values()
            .filter(|r| rec_retirable(&r.state))
            .map(|r| {
                (
                    r.finished_ms.or(Some(r.created_ms)),
                    r.created_ms,
                    r.id.clone(),
                )
            })
            .collect();
        if terminals.len() <= TERMINAL_RECORD_CAP {
            return;
        }
        terminals.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)).then_with(|| a.2.cmp(&b.2)));
        let excess = terminals.len() - TERMINAL_RECORD_CAP;
        for (_, _, id) in &terminals[..excess] {
            map.remove(id);
        }
        tracing::debug!(retired = excess, cap = TERMINAL_RECORD_CAP, "终态记录超限,退役最老记录");
    }

    /// 内存面 → 文件镜像(原子写):id 升序整形落盘,保证文件内容确定
    /// (便于人读/排障)。**持 map 锁全程调用**(写频极低:建队/翻转/
    /// 节流回写,毫秒级临时件 rename 不构成热点)——写者们(跨线程的
    /// upsert/remove)在锁上串行,同一时刻至多一个临时件在写,并发双写
    /// 撕裂 `tasks.json.tmp` 的窗口由此闭合。落盘失败**不停摆**(模块
    /// 注释「写失败活力面」):内存权威面不受影响,每次写操作原样重试,
    /// 恢复后全量补写;可见性走限频记账([`note_failure`]):状态翻转才
    /// warn 一条,恢复 info 一条,持续失败不逐写刷屏。序列化失败(理论
    /// 不可达:纯基本类型面)同样跳过本次落盘,不写半个文件。
    fn flush_locked(&self, map: &HashMap<String, TaskRec>) {
        let mut recs: Vec<TaskRec> = map.values().cloned().collect();
        recs.sort_by(|a, b| a.id.cmp(&b.id));
        let dump = StoreDump {
            // 水位在 upsert 里先于本函数前推(fetch_max),此处读到的值
            // ≥ 一切此前落盘值(写者持同一把锁串行)
            next_incoming_id: self.incoming_wm.load(Ordering::Relaxed),
            recs,
        };
        let body = match serde_json::to_string_pretty(&dump) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(%err, "tasks.json 序列化失败,跳过本次落盘");
                return;
            }
        };
        match (self.writer)(&self.home.join(TASKS_FILE), &body) {
            Ok(()) => {
                if lock_ok(&self.failed).take().is_some() {
                    // 恢复即愈合:本次成功写 = 当前内存面全量,中断期新增
                    // 记录一并落盘(镜像不再停留旧断面)
                    tracing::info!("tasks.json 落盘恢复(中断期镜像已全量补写)");
                }
            }
            Err(err) => {
                let msg = err.to_string();
                if note_failure(&mut lock_ok(&self.failed), &msg) {
                    tracing::warn!(
                        %err,
                        "tasks.json 落盘失败(内存面不受影响,持续重试;恢复后全量补写)"
                    );
                }
            }
        }
    }
}

/// 重启恢复标记(两形态解析共用):非终态(running/paused/queued/
/// publishing)→ interrupted,终态(done/failed/cancelled)照旧。
fn mark_interrupted(recs: Vec<TaskRec>, map: &mut HashMap<String, TaskRec>) {
    *map = recs.into_iter().map(|r| (r.id.clone(), r)).collect();
    for r in map.values_mut() {
        if !matches!(r.state.as_str(), "done" | "failed" | "cancelled") {
            r.state = "interrupted".into();
        }
    }
}

/// 损坏兜底(解析失败/非 UTF-8 内容共用):改名 `.bad-<ts>` 留底,调用方
/// 从空开始(毫秒戳防连坏多次互相覆盖;备份失败不拦启动,仅留痕)。
fn backup_corrupt(home: &Path, path: &Path, what: &str, err: &dyn std::fmt::Display) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let bad = home.join(format!("{TASKS_FILE}.bad-{ts}"));
    match std::fs::rename(path, &bad) {
        Ok(()) => tracing::warn!(
            err = %err,
            bad = %bad.display(),
            "tasks.json 损坏({what}),已备份后从空开始"
        ),
        Err(rerr) => tracing::warn!(
            err = %err,
            %rerr,
            path = %path.display(),
            "tasks.json 损坏且备份失败,按空表继续"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ① 存取往返:upsert/get/remove 的基础语义 + list 按 id 字典序升序
    ///(`<dev>:<seq>` 字符串序,`devA:10` 排在 `devA:2` 之前)+ 落盘镜像
    /// 可被再次 load 读回。
    #[test]
    fn roundtrip_upsert_remove_list() {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::load(dir.path());
        let a = TaskRec {
            id: "devA:2".into(),
            kind: "upload".into(),
            state: "running".into(),
            ..Default::default()
        };
        let b = TaskRec {
            id: "devA:10".into(),
            kind: "download".into(),
            // 终态:重载不做中断改写(load 的非终态 → interrupted 是测试 ②
            // 的领地,此处只验「文件内容原样往返」)
            state: "done".into(),
            ..Default::default()
        };
        store.upsert(&b);
        store.upsert(&a);

        // get 往返:存在命中、缺失 None
        assert_eq!(store.get("devA:2").unwrap().state, "running");
        assert_eq!(store.get("devA:2").unwrap().kind, "upload");
        assert!(store.get("missing:1").is_none());

        // list 升序 = id 字典序(字符串序,非数字序)
        let ids: Vec<String> = store.list().into_iter().map(|r| r.id).collect();
        assert_eq!(
            ids,
            vec!["devA:10".to_string(), "devA:2".to_string()],
            "list 必须按 id 字典序升序"
        );

        // remove:命中 true、二次 false、get/list 随之收敛
        assert!(store.remove("devA:2"));
        assert!(!store.remove("devA:2"), "重复 remove 报 false");
        assert!(store.get("devA:2").is_none());
        assert_eq!(store.list().len(), 1);

        // 落盘镜像:重开一个 store 从 tasks.json 读回剩余记录
        let store2 = TaskStore::load(dir.path());
        assert_eq!(store2.list().len(), 1, "落盘镜像须可读回");
        assert_eq!(store2.get("devA:10").unwrap().state, "done");
    }

    /// ② 重启中断标记:load 把非终态(running/paused/queued/publishing)
    /// 一律标 `interrupted`;终态(done/failed/cancelled)照旧不动。
    /// publishing(P4a 收件发布窗「保存中」,T1 增设)顺钉:非终态语义
    /// 重启即中断——暂存与记录在盘,重发走 republish 分区(T2,spec §6.3)。
    #[test]
    fn load_marks_nonterminal_interrupted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("tasks.json"),
            r#"[
            {"id":"a:1","kind":"upload","state":"running"},
            {"id":"a:2","kind":"upload","state":"paused"},
            {"id":"a:3","kind":"download","state":"queued"},
            {"id":"a:4","kind":"upload","state":"done"},
            {"id":"a:5","kind":"upload","state":"failed"},
            {"id":"a:6","kind":"upload","state":"cancelled"},
            {"id":"a:7","kind":"download","state":"publishing"}
        ]"#,
        )
        .unwrap();
        let store = TaskStore::load(dir.path());
        let states: Vec<String> = store.list().into_iter().map(|r| r.state).collect();
        assert_eq!(
            states,
            vec![
                "interrupted",
                "interrupted",
                "interrupted",
                "done",
                "failed",
                "cancelled",
                "interrupted"
            ],
            "非终态一律 interrupted(含 publishing),终态原样保留(list 按 id 升序)"
        );
    }

    /// ③ 损坏兜底:tasks.json 解析失败 → load 从空表开始,原文件改名
    /// `tasks.json.bad-<ts>` 备份留底(原内容完整保留,原路径不再有文件)。
    #[test]
    fn corrupt_json_backs_up_and_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tasks.json"), "{not json").unwrap();
        let store = TaskStore::load(dir.path());
        assert!(store.list().is_empty(), "损坏后从空表开始");
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        let bads: Vec<&String> = names
            .iter()
            .filter(|n| n.starts_with("tasks.json.bad-"))
            .collect();
        assert_eq!(bads.len(), 1, "应恰好一个 .bad-<ts> 备份: {names:?}");
        let backed = std::fs::read_to_string(dir.path().join(bads[0])).unwrap();
        assert_eq!(backed, "{not json", "备份须原样保留损坏内容");
        assert!(
            !dir.path().join("tasks.json").exists(),
            "原路径不再有损坏文件(已让位给备份)"
        );
    }

    /// ③b 非 UTF-8 损坏(read_to_string 的 InvalidData)与解析失败同一
    /// 兜底面:备份 `.bad-<ts>` 后从空开始,不留「静默清空、原件无备份」
    /// 的口子(终审修复 Minor#5)。
    #[test]
    fn corrupt_non_utf8_backs_up_and_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let raw: &[u8] = &[0xff, 0xfe, 0x00, 0x01, 0x7b];
        std::fs::write(dir.path().join("tasks.json"), raw).unwrap();
        let store = TaskStore::load(dir.path());
        assert!(store.list().is_empty(), "非 UTF-8 损坏后从空表开始");
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        let bads: Vec<&String> = names
            .iter()
            .filter(|n| n.starts_with("tasks.json.bad-"))
            .collect();
        assert_eq!(bads.len(), 1, "应恰好一个 .bad-<ts> 备份: {names:?}");
        let backed = std::fs::read(dir.path().join(bads[0])).unwrap();
        assert_eq!(backed, raw, "备份须原样保留损坏字节");
        assert!(
            !dir.path().join("tasks.json").exists(),
            "原路径不再有损坏文件(已让位给备份)"
        );
    }

    /// ④ 前向兼容:旧版/残缺记录(缺 name 等)加载取类型零值——
    /// name = "",不拒载、不 panic。
    #[test]
    fn partial_old_record_defaults_name_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("tasks.json"),
            r#"[{"id":"old:1","kind":"upload","state":"done"}]"#,
        )
        .unwrap();
        let store = TaskStore::load(dir.path());
        let rec = store.get("old:1").unwrap();
        assert_eq!(rec.name, "", "缺 name 字段 → 前向兼容缺省空串");
        assert_eq!(rec.state, "done", "在场的字段照常读出");
        assert!(rec.src_paths.is_empty());
        assert_eq!(rec.finished_ms, None);
    }

    /// ⑤ 匿名卡计数水位(v3p6 spec §5.1-4,计划 Review Focus 4):incoming
    /// id 是裸数字自增域(重启归零即撞号)→ 水位随 tasks.json 持久。
    /// ① 旧裸数组(无新字段)加载 → 自记录推导 `max(job_seq)+1`(裸数字与
    ///    `<dev>:<seq>` 命名空间两型同池推导,空表 0)——与 Jobs::new 的
    ///    播种口径一致;
    /// ② 新包装形态 `{next_incoming_id, recs}` 水位照读;字段值小于记录
    ///    推导值(手改/外来文件)时折叠取大;
    /// ③ upsert 按记录 id 前推水位并随同一次落盘持久(重载读回)。
    #[test]
    fn incoming_watermark_legacy_derive_and_roundtrip() {
        // 无文件:空表起步,水位 0(Jobs::new 落到地板 1,首铸 id 1)
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::load(dir.path());
        assert_eq!(store.incoming_watermark(), 0, "无文件 → 水位 0");

        // ① 旧裸数组:无 next_incoming_id 字段 → 推导 max(job_seq)+1 = 6
        std::fs::write(
            dir.path().join("tasks.json"),
            r#"[
            {"id":"devA:5","kind":"download","state":"done"},
            {"id":"5","kind":"incoming","state":"done"}
        ]"#,
        )
        .unwrap();
        let store = TaskStore::load(dir.path());
        assert_eq!(
            store.incoming_watermark(),
            6,
            "旧文件缺字段 → 自记录推导(裸数字与命名空间两型同池)"
        );

        // ② 新包装形态:水位照读(recs 同载)
        std::fs::write(
            dir.path().join("tasks.json"),
            r#"{"next_incoming_id": 42, "recs": [{"id":"5","kind":"incoming","state":"done"}]}"#,
        )
        .unwrap();
        let store = TaskStore::load(dir.path());
        assert_eq!(store.incoming_watermark(), 42, "包装形态水位照读");
        assert_eq!(store.get("5").unwrap().kind, "incoming", "包装 recs 同载");

        // ②b 字段小于记录推导值(手改/外来文件)→ 折叠取大
        std::fs::write(
            dir.path().join("tasks.json"),
            r#"{"next_incoming_id": 3, "recs": [{"id":"9","kind":"incoming","state":"done"}]}"#,
        )
        .unwrap();
        let store = TaskStore::load(dir.path());
        assert_eq!(store.incoming_watermark(), 10, "水位与记录推导折叠取大");

        // ③ upsert 按记录 id 前推水位("devB:20" → 21)并落盘持久
        store.upsert(&TaskRec {
            id: "devB:20".into(),
            kind: "download".into(),
            state: "done".into(),
            ..Default::default()
        });
        assert_eq!(store.incoming_watermark(), 21, "upsert 按记录 id 前推水位");
        let store2 = TaskStore::load(dir.path());
        assert_eq!(
            store2.incoming_watermark(),
            21,
            "水位随落盘持久,重载读回"
        );
    }

    /// ⑥ 终态持久上限 100(spec §5.1-4「终态持久上限 100」):终态记录
    /// 超限按终态时刻退役最老的——done/failed/cancelled/interrupted 同池
    /// 计数,**含 incoming**;非终态(running/paused/queued/publishing)
    /// 永不退役;水位单调(退役不回退);退役随落盘持久。
    #[test]
    fn terminal_cap_retires_oldest_including_incoming() {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::load(dir.path());
        // 100 条终态:upload/incoming 两 kind 交错,终态时刻随 id 递增;
        // id "1" 特意写 interrupted(恢复标记面)——同池退役的证词
        for i in 1..=100u64 {
            store.upsert(&TaskRec {
                id: i.to_string(),
                kind: if i % 2 == 0 { "incoming" } else { "upload" }.into(),
                name: format!("卡{i}"),
                state: if i == 1 { "interrupted".into() } else { "done".into() },
                created_ms: i * 1000,
                finished_ms: Some(i * 1000),
                ..Default::default()
            });
        }
        // 两条非终态:与终态共存,任何退役都不得动它们
        for (id, kind, state) in [
            ("live:1", "upload", "running"),
            ("live:2", "incoming", "paused"),
        ] {
            store.upsert(&TaskRec {
                id: id.into(),
                kind: kind.into(),
                state: state.into(),
                ..Default::default()
            });
        }

        // 第 101 条终态落定 → 最老终态(id "1")退役,终态总量收敛 100
        store.upsert(&TaskRec {
            id: "101".into(),
            kind: "incoming".into(),
            name: "新卡".into(),
            state: "done".into(),
            created_ms: 101_000,
            finished_ms: Some(101_000),
            ..Default::default()
        });

        assert!(store.get("1").is_none(), "最老终态记录(interrupted)被退役");
        assert_eq!(store.get("2").unwrap().name, "卡2", "次老终态保留");
        assert_eq!(store.get("101").unwrap().name, "新卡", "最新终态保留");
        assert!(
            store.get("live:1").is_some() && store.get("live:2").is_some(),
            "非终态永不退役"
        );
        let count_terminal = |recs: &[TaskRec]| {
            recs.iter()
                .filter(|r| {
                    matches!(
                        r.state.as_str(),
                        "done" | "failed" | "cancelled" | "interrupted"
                    )
                })
                .count()
        };
        assert_eq!(count_terminal(&store.list()), 100, "终态记录收敛到上限 100");
        assert!(
            store.incoming_watermark() >= 102,
            "水位单调:退役不回退,upsert 前推照常"
        );

        // 落盘镜像:重开读回,收敛结果持久(重载把两条非终态翻 interrupted
        // ——load 的重启标记语义,测试 ② 的领地;总记录 = 100 终态 + 2 条)
        let store2 = TaskStore::load(dir.path());
        assert_eq!(
            store2.list().len(),
            102,
            "退役随落盘持久(100 终态 + 2 非终态)"
        );
        assert!(store2.get("1").is_none(), "退役不在镜像");
        assert_eq!(
            store2.get("live:2").unwrap().state,
            "interrupted",
            "非终态镜像保留(重启标记照常)"
        );
    }

    /// ⑦ 限频记账状态机(Task 24,[`note_failure`] 纯函数钉死):None→
    /// Some 首现记一条(true);同一错误持续不记(false);错误文案变化
    /// 再记(true);恢复清空 None 后再失败又是首现(true)。「日志只随
    /// 状态变化出」的限频面由此锚定——现场票的诉求是可见性,不是刷屏。
    #[test]
    fn note_failure_logs_once_per_state_change() {
        let mut failed = None;
        assert!(note_failure(&mut failed, "err A"), "健康→失败首现必须记");
        assert_eq!(failed.as_deref(), Some("err A"));
        assert!(!note_failure(&mut failed, "err A"), "同一错误持续不记");
        assert!(note_failure(&mut failed, "err B"), "错误文案变化必须记");
        assert_eq!(failed.as_deref(), Some("err B"));
        assert!(!note_failure(&mut failed, "err B"), "变化后同错持续不记");
        failed = None; // 恢复(调用方清空并记 info)
        assert!(note_failure(&mut failed, "err B"), "恢复后再失败=首现必须记");
    }

    /// ⑧ 写失败→恢复全量补写(Task 24 现场票的活力性质锚):落盘持续
    /// 失败期间——①内存权威面照常读写(upsert/get/remove 不受影响);
    /// ②镜像文件不动(mtime 冻结的现场形态:file 压根不出现);③失败
    /// 状态面 Some(限频记账在场)。失败解除后的下一次写 = 全量补写:
    /// 中断期内存面新增的记录**全部**落盘(镜像自愈,不停摆),失败状态
    /// 回 None,重载读回闭合。这正是「持久化不能无声断」的回归红线:
    /// 写失败永不解除写路径,恢复永不丢中断期账面。
    #[test]
    fn flush_failure_resumes_with_full_mirror() {
        use std::sync::atomic::AtomicBool;

        let dir = tempfile::tempdir().unwrap();
        // 注错阀:true = 落盘恒失败(不触盘);false = 真写(write_atomic)。
        let gate = Arc::new(AtomicBool::new(true));
        let g = gate.clone();
        let writer: FlushFn = Arc::new(move |p, body| {
            if g.load(Ordering::Relaxed) {
                Err(std::io::Error::other("injected write failure"))
            } else {
                write_atomic(p, body).map_err(|e| std::io::Error::other(e.to_string()))
            }
        });
        let store = TaskStore::load_with_writer(dir.path(), writer);
        assert_eq!(store.flush_failed(), None, "健康起步:失败面空");

        let a = TaskRec {
            id: "devA:1".into(),
            kind: "download".into(),
            state: "running".into(),
            ..Default::default()
        };
        let b = TaskRec {
            id: "devA:2".into(),
            kind: "incoming".into(),
            state: "done".into(),
            ..Default::default()
        };
        // 失败期:内存面照常,镜像不动
        store.upsert(&a);
        assert_eq!(store.get("devA:1").unwrap().state, "running", "内存权威面不受落盘失败影响");
        assert!(
            !dir.path().join("tasks.json").exists(),
            "落盘持续失败:镜像文件不得出现(mtime 冻结的现场形态)"
        );
        let fail_msg = store.flush_failed().expect("首败必须记账");
        assert!(fail_msg.contains("injected"), "失败面=最近错误文案: {fail_msg}");
        // 同一错误持续:记账不变(限频,不逐写翻转)
        store.upsert(&b);
        assert_eq!(store.get("devA:2").unwrap().state, "done", "失败期内存面继续照常");
        assert_eq!(
            store.flush_failed().as_deref(),
            Some(fail_msg.as_str()),
            "持续失败:失败面原样(状态未翻转)"
        );
        // remove 也走同一落盘面:失败期照常生效于内存面
        assert!(store.remove("devA:2"), "失败期 remove 内存面照常命中");

        // 恢复:失败解除后的下一次写 = 全量补写(devA:1 在场——中断期账面不丢)
        gate.store(false, Ordering::Relaxed);
        store.upsert(&TaskRec {
            id: "devA:3".into(),
            kind: "download".into(),
            state: "done".into(),
            ..Default::default()
        });
        assert_eq!(store.flush_failed(), None, "恢复:失败面清空");
        let healed = std::fs::read_to_string(dir.path().join("tasks.json")).unwrap();
        assert!(healed.contains("devA:1"), "中断期记录随全量补写落盘: {healed}");
        assert!(healed.contains("devA:3"), "恢复写记录在场: {healed}");
        assert!(
            !healed.contains("devA:2"),
            "中断期被 remove 的记录不复活(镜像=当前内存面): {healed}"
        );
        // 重载闭合:全量补写面可读回(非终态 devA:1 重启标记 interrupted,
        // load 的恢复语义;总数 2 = devA:1 + devA:3)
        let store2 = TaskStore::load(dir.path());
        let ids: Vec<String> = store2.list().into_iter().map(|r| r.id).collect();
        assert_eq!(ids.len(), 2, "重载读回全量补写面: {ids:?}");
        assert!(ids.contains(&"devA:1".to_string()) && ids.contains(&"devA:3".to_string()));
        assert_eq!(store2.get("devA:1").unwrap().state, "interrupted", "非终态重启标记照常");
    }
}
