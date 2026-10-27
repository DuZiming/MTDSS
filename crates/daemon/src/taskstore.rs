//! tasks.json:传输任务记录持久层(v2-P2 Task 3)。
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
//! 内存面 `Mutex<HashMap>` 为权威,文件是镜像;`load` 只在启动调一次,
//! 并把非终态(running/paused/queued/publishing)一律标记 interrupted——daemon
//! 死了传输必然断了,重启后置「中断」等用户重新发起(终态 done/
//! failed/cancelled 照旧)。持久层不含任何驱动/注册表逻辑:jobs 的
//! 活跃面(Task 4+)经 upsert/get/list 镜像进来,两套数据不互侵入。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::jobs::lock_ok;
use crate::session::write_atomic;

/// home 下任务记录文件名。
const TASKS_FILE: &str = "tasks.json";

/// 单条传输任务记录(brief 字段面钉死,Task 4/6/7/8 消费)。全字段
/// serde default:旧/残记录缺字段即取类型零值,前向兼容加载。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)] // 全字段缺省(容器级:等价于逐字段 #[serde(default)])
pub(crate) struct TaskRec {
    /// 作业 id(`<dev>:<seq>` 命名空间;重发翻新同记录换新 job_id)。
    pub id: String,
    /// 作业类型(upload|download|remote_copy)。
    pub kind: String,
    /// 显示名(首项名)。
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
    /// 数据目录(落盘 `{home}/tasks.json`)。
    home: PathBuf,
}

impl TaskStore {
    /// 启动装配:读 `{home}/tasks.json` 入内存,并把非终态标 interrupted
    /// (重启恢复,见模块注释)。文件缺失/读取失败/解析失败一律从空表
    /// 继续(解析失败与非 UTF-8 内容先备份 `.bad-<ts>` 留底);不返回
    /// Result——记录是可丢面板数据,任何异常都不拦 daemon 启动。
    pub(crate) fn load(home: &Path) -> Self {
        let path = home.join(TASKS_FILE);
        let mut map = HashMap::new();
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
                Ok(recs) => {
                    map = recs.into_iter().map(|r| (r.id.clone(), r)).collect();
                    // 重启恢复:非终态(running/paused/queued/publishing)
                    // → interrupted,终态(done/failed/cancelled)照旧。
                    for r in map.values_mut() {
                        if !matches!(r.state.as_str(), "done" | "failed" | "cancelled") {
                            r.state = "interrupted".into();
                        }
                    }
                }
                Err(err) => backup_corrupt(home, &path, "解析失败", &err),
            },
        }
        Self {
            map: Mutex::new(map),
            home: home.to_path_buf(),
        }
    }

    /// 插入/覆盖一条记录并即时落盘(状态翻转即持久,spec §9)。落盘持
    /// 同一把 map 锁全程(见 [`TaskStore::flush_locked`]):不同线程的并发
    /// 写(worker 进度回写 / RPC 状态翻转)在锁上串行,不复现两写者交错
    /// 写坏 `tasks.json.tmp` 的撕裂窗。
    pub(crate) fn upsert(&self, rec: &TaskRec) {
        let mut map = lock_ok(&self.map);
        map.insert(rec.id.clone(), rec.clone());
        self.flush_locked(&map);
    }

    /// 按 id 删除记录并即时落盘。命中 true;不存在 false(不空转落盘)。
    /// 消费者:reinitiate 的翻新摘旧(v2-P2 Task 6)与 Jobs::clear
    /// (transfer.clear,Task 7)。
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

    /// 全量快照,id 字典序升序(`<dev>:<seq>` 字符串序)。
    pub(crate) fn list(&self) -> Vec<TaskRec> {
        let mut recs: Vec<TaskRec> = lock_ok(&self.map).values().cloned().collect();
        recs.sort_by(|a, b| a.id.cmp(&b.id));
        recs
    }

    /// 内存面 → 文件镜像(原子写):id 升序整形落盘,保证文件内容确定
    /// (便于人读/排障)。**持 map 锁全程调用**(写频极低:建队/翻转/
    /// 节流回写,毫秒级临时件 rename 不构成热点)——写者们(跨线程的
    /// upsert/remove)在锁上串行,同一时刻至多一个临时件在写,并发双写
    /// 撕裂 `tasks.json.tmp` 的窗口由此闭合。落盘失败仅 warn——内存权威
    /// 面不受影响,下次写操作自然重试;序列化失败(理论不可达:纯基本
    /// 类型面)同样跳过本次落盘,不写半个文件。
    fn flush_locked(&self, map: &HashMap<String, TaskRec>) {
        let mut recs: Vec<TaskRec> = map.values().cloned().collect();
        recs.sort_by(|a, b| a.id.cmp(&b.id));
        let body = match serde_json::to_string_pretty(&recs) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(%err, "tasks.json 序列化失败,跳过本次落盘");
                return;
            }
        };
        if let Err(err) = write_atomic(&self.home.join(TASKS_FILE), &body) {
            tracing::warn!(%err, "tasks.json 落盘失败(内存面不受影响,下次写操作重试)");
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
}
