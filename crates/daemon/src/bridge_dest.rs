//! 收件箱目的地(v2-P3 T6):Android 收件箱的 [`DestIo`] 实现——fd 桥形态。
//!
//! 磁盘面拆两半:
//! - **记录**:`{home}/inbox-records/<sha256(rel)>.json`(intake 统一
//!   schema/键空间/`.json.tmp`+rename 原子写,见 `intake` 模块文档;与
//!   FsDestIo 的 `recv_dir/.mtdss` 键空间天然隔离);
//! - **数据与终件**:provider(App)侧——暂存 fd 经 `fs.request_dest` 事件
//!   请求 `{request_id, rel, size, resume}`,App 以 fdpass `0x02 dest_fd`
//!   应答(无 fd = 拒开);finalize 提交 = `0x03 dest_commit
//!   {request_id, done:true}` **原路**送回(App 揭开 pending 项,如
//!   MediaStore IS_PENDING=0),轮收尾的失败路径送 `done:false`(App 清
//!   pending 暂存项)。
//!
//! cfg 无关(记录簿记 + 通道调用面均跨平台;fdpass 通道本身 unix 才有,
//! Windows 下 resolver 占位面把请求收口为「目的地不可用」)——host 单测
//! 可直接构造。
//!
//! 语义要点(对照 `DestIo` trait 文档):
//! - `partial_exists`:恒按记录面回答(记录在场即素材在场的乐观判定;
//!   provider 侧真缺席由 `open_writer(resume=true)` 的拒开降级兜底);
//! - **resume 拒开 = 素材已失**:清除陈旧记录后按 Err 上抛,轮失败 →
//!   发送端重试轮对账无记录 → **全新重收**(resume/full 决策随 provider
//!   应答);拒开发生在 resume=false(全新收)则直接任务失败;
//! - `final_state`:记录面回答(`0x03 done:true` 落定的记录才是 Skip 的
//!   依据;provider 侧无在场查询协议,记录即唯一本地事实);
//! - 轮收尾(成功/失败/取消一并):已交付未提交的请求逐条 `0x03
//!   done:false`——成功路径 commit 恰一次 done:true,失败路径由 Drop
//!   收尾兜底,每条请求恰一次 0x03。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use futures_util::future::{BoxFuture, FutureExt};
use mtdss_core::transport::intake::{record_key, Committed, DestIo};
use mtdss_proto::manifest::FileEntry;

#[cfg(unix)]
use anyhow::Context as _;
#[cfg(unix)]
use crate::jobs::DestOpenError;
use crate::jobs::{lock_ok, DestResolver};

/// 收件箱记录目录名(home 下;与 `.mtdss`/`tasks.json` 等 home 级键空间
/// 并列,互不触碰)。
const RECORDS_DIR: &str = "inbox-records";

/// 收件箱目的地:记录在 `{home}/inbox-records`,数据/终件在 provider 侧
/// (fd 桥)。每接收轮一个实例(resident loop 哨兵路由处构造);跨轮的
/// 断点续传靠记录面(对账/PartWriter 全在 intake,与本实现无关)。
pub(crate) struct BridgeDestIo {
    home: PathBuf,
    resolver: Arc<DestResolver>,
    /// rel → 在途 request_id(已交付 fd、尚未 commit)。commit 弹出并送
    /// `0x03 done:true`;轮收尾(Drop)逐条送 `0x03 done:false`。std
    /// Mutex:临界区内无 await。
    open: std::sync::Mutex<HashMap<String, u64>>,
}

impl BridgeDestIo {
    pub(crate) fn new(home: impl Into<PathBuf>, resolver: Arc<DestResolver>) -> Self {
        Self {
            home: home.into(),
            resolver,
            open: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// 记录文件路径({records_dir}/<sha256(rel)>.json;intake 的键空间)。
    fn record_file(&self, rel: &str) -> PathBuf {
        self.records_dir().join(format!("{}.json", record_key(rel)))
    }

    /// open_writer 的 unix 本体:fs.request_dest 请求 → fd → set_len 预
    /// 分配 → 在途登记。resume 拒开降级(清陈旧记录)的完整语义见模块
    /// 文档。
    #[cfg(unix)]
    fn open_writer_unix<'a>(
        &'a self,
        entry: &'a FileEntry,
        resume: bool,
    ) -> BoxFuture<'a, Result<tokio::fs::File>> {
        async move {
            // 记录目录 ensure 先于请求:全新收随开箱即写记录(trait 契约:
            // open_writer 返回前 records_dir() 必须已可用)。
            let records = self.records_dir();
            std::fs::create_dir_all(&records).with_context(|| records.display().to_string())?;
            let rel = entry.rel_path.as_str();
            let (request_id, fd) = match self.resolver.request(rel, entry.size, resume).await {
                Ok(v) => v,
                Err(DestOpenError::Refused(reason)) => {
                    if resume {
                        // 素材已失:清除陈旧记录(与 intake 对账的陈旧清理
                        // 同一语义——记录指向的续传素材不复存在),轮失败
                        // 后发送端重试轮对账无记录 → 全新重收。
                        let rec = self.record_file(rel);
                        let _ = std::fs::remove_file(&rec);
                        return Err(anyhow!(
                            "目的地不可用: {reason}(rel={rel}),断点已清除,重试将按全新接收"
                        ));
                    }
                    return Err(anyhow!("目的地不可用: {reason}(rel={rel})"));
                }
                Err(DestOpenError::Unavailable(m)) => {
                    return Err(anyhow!("目的地不可用: {m}(rel={rel})"));
                }
            };
            // fd → tokio 句柄。可读可写由 provider 保证(finalize 的整文件
            // 读回校验需要读权);set_len 预分配至清单尺寸(截断上次异常
            // 退出的素材,resume 形态同尺寸幂等)。
            let file = tokio::fs::File::from(std::fs::File::from(fd));
            if let Err(e) = file.set_len(entry.size).await {
                // 预分配失败:fd 无法充当续写句柄——0x03 done:false 即时
                // 回收(App 清 pending 项),不等待轮收尾。
                self.resolver.commit_dest(request_id, false);
                return Err(anyhow!(e)).with_context(|| format!("preallocate {rel}"));
            }
            lock_ok(&self.open).insert(rel.to_string(), request_id);
            Ok(file)
        }
        .boxed()
    }
}

impl DestIo for BridgeDestIo {
    fn records_dir(&self) -> PathBuf {
        self.home.join(RECORDS_DIR)
    }

    fn final_state(&self, final_rel: &str) -> Option<u64> {
        // 记录面回答:done 且 final_rel 吻合 → Some(记录 size)。对账处
        // (intake::plan_for)与清单条目比对 size/哈希/块数,陈旧记录已在
        // 前置校验剔除;provider 侧在场无查询协议——0x03 done:true 落定
        // 的记录即唯一本地事实。读失败/字段不符一律 None(不 panic)。
        let bytes = std::fs::read(self.record_file(final_rel)).ok()?;
        let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        if v["rel"].as_str() != Some(final_rel) {
            return None;
        }
        if v["done"].as_bool() != Some(true) {
            return None;
        }
        if v["final_rel"].as_str() != Some(final_rel) {
            return None;
        }
        v["size"].as_u64()
    }

    fn partial_exists(&self, rel: &str) -> bool {
        // 恒按记录面回答(trait 契约的乐观判定面):记录在场即视为素材
        // 在场,provider 侧真缺席由 open_writer(resume=true) 的拒开
        // 降级兜底(拒开 → 清记录 → 重试轮全新重收)。
        self.record_file(rel).exists()
    }

    fn discard_partial(&self, _rel: &str) {
        // provider 侧暂存无清理协议(0x03 done:false 的尽力而为面);记录
        // 删除由调用方(cleanup_rels/对账)承担。
    }

    fn prepare_overwrite(&self, _rel: &str) {
        // provider 侧无本地目标树(MediaStore 侧自决定名),无清障面。
    }

    fn free_bytes(&self) -> u64 {
        // provider 容量未知——按「无限」处理(trait 契约:查询失败不因
        // 未知空间误拒)。
        u64::MAX
    }

    fn open_writer<'a>(
        &'a self,
        entry: &'a FileEntry,
        resume: bool,
    ) -> BoxFuture<'a, Result<tokio::fs::File>> {
        // fd 交付面 unix 专属(fdpass 通道 + std::os::fd 类型面);Windows
        // 收件箱轮不存在(哨兵路由 cfg(unix) 门内),本臂仅为 cfg 无关
        // 编译面存在的占位收口。
        #[cfg(unix)]
        {
            self.open_writer_unix(entry, resume)
        }
        #[cfg(not(unix))]
        {
            let _ = (entry, resume);
            async { Err(anyhow!("目的地不可用: 本平台无 fd 通道")) }.boxed()
        }
    }

    fn commit<'a>(
        &'a self,
        rel: &'a str,
        _part: &'a mut tokio::fs::File,
    ) -> BoxFuture<'a, Result<Committed>> {
        async move {
            // 0x03 dest_commit(原路连接)——provider 落定终件。句柄仍由
            // PartWriter 持有至记录落盘后释放(T5 签名押注:impl 不先关
            // fd 再发 dest_commit,SCM_RIGHTS-dup 模型下两侧并行无害)。
            let request_id = lock_ok(&self.open)
                .remove(rel)
                .ok_or_else(|| anyhow!("收件箱提交失序: {rel} 无在途请求"))?;
            if !self.resolver.commit_dest(request_id, true) {
                // 送达失败(连接已亡):提交按失败上抛,记录保持 partial,
                // 重试轮重开重提交(断电/断连同构的可恢复状态)。
                return Err(anyhow!(
                    "目的地不可用: dest_commit 未能送达(rel={rel})"
                ));
            }
            Ok(Committed {
                // 展示面虚拟路径:终件在 provider 侧存储,daemon 无真实
                // 路径——仅进通知文案/落盘日志,不作 IO。final_rel = rel
                // (provider 侧终名,无「 (n)」冲突改名面)。
                path: PathBuf::from(format!("inbox/{rel}")),
                final_rel: rel.to_string(),
            })
        }
        .boxed()
    }
}

impl Drop for BridgeDestIo {
    /// 轮收尾(成功/失败/取消一并):已交付未提交的请求逐条 `0x03
    /// done:false`(provider 清 pending 暂存项)。尽力而为——连接已亡即
    /// 丢弃(发送端重试轮自会重新请求,provider 侧 pending 项由其自身
    /// 生命周期兜底)。与 commit 的恰一次配对由 open 表的弹出语义保证。
    fn drop(&mut self) {
        for (rel, request_id) in lock_ok(&self.open).drain() {
            if !self.resolver.commit_dest(request_id, false) {
                tracing::debug!(rel = %rel, request_id, "收件箱失败收尾 0x03 未送达(连接已亡,忽略)");
            }
        }
    }
}

// ---------- 单元测试(记录面;fd 交付面由 inbox_bridge_e2e 覆盖) ----------

#[cfg(test)]
mod tests {
    use super::*;
    use mtdss_core::transport::intake::INBOX_SENTINEL;

    /// 测试 resolver:真实 DestResolver + 测试自持的事件接收端(broadcast
    /// 通道驱动拒开路径,不触 fdpass)。
    fn resolver() -> (Arc<DestResolver>, tokio::sync::broadcast::Receiver<mtdss_ipc::EventFrame>) {
        let (events, rx) = tokio::sync::broadcast::channel(16);
        (Arc::new(DestResolver::new(events)), rx)
    }

    fn dest(home: &std::path::Path) -> BridgeDestIo {
        let (resolver, _rx) = resolver();
        BridgeDestIo::new(home.to_path_buf(), resolver)
    }

    /// 落一份记录(字段面与 intake FileRecord 一致)。
    fn put_record(d: &BridgeDestIo, rel: &str, v: serde_json::Value) {
        std::fs::create_dir_all(d.records_dir()).unwrap();
        std::fs::write(d.record_file(rel), serde_json::to_vec(&v).unwrap()).unwrap();
    }

    /// records_dir 布局:{home}/inbox-records(与 FsDestIo 的
    /// recv_dir/.mtdss 键空间隔离)。
    #[test]
    fn records_dir_is_home_scoped() {
        let home = tempfile::tempdir().unwrap();
        let d = dest(home.path());
        assert_eq!(d.records_dir(), home.path().join("inbox-records"));
    }

    /// final_state:done + final_rel 吻合 → Some(记录 size);未完成/
    /// final_rel 不符/记录缺席 → None(Skip 判据的唯一查询面)。
    #[test]
    fn final_state_reads_done_record() {
        let home = tempfile::tempdir().unwrap();
        let d = dest(home.path());
        put_record(
            &d,
            "docs/a.bin",
            serde_json::json!({
                "rel": "docs/a.bin", "size": 42,
                "sha256": format!("{:064}", 0), "chunk_count": 1,
                "bitmap": "01", "done": true, "final_rel": "docs/a.bin",
            }),
        );
        assert_eq!(d.final_state("docs/a.bin"), Some(42), "done 记录回答 size");
        assert_eq!(d.final_state("missing.bin"), None, "记录缺席 → None");

        // final_rel 不符(该记录的终名不是查询键)→ None
        put_record(
            &d,
            "docs/b.bin",
            serde_json::json!({
                "rel": "docs/b.bin", "size": 7,
                "sha256": format!("{:064}", 0), "chunk_count": 1,
                "bitmap": "01", "done": true, "final_rel": "docs/改名.bin",
            }),
        );
        assert_eq!(d.final_state("docs/b.bin"), None, "final_rel 不符 → None");

        // 未完成(partial)→ None
        put_record(
            &d,
            "docs/c.bin",
            serde_json::json!({
                "rel": "docs/c.bin", "size": 9,
                "sha256": format!("{:064}", 0), "chunk_count": 1,
                "bitmap": "00", "done": false, "final_rel": serde_json::Value::Null,
            }),
        );
        assert_eq!(d.final_state("docs/c.bin"), None, "partial 记录 → None");
    }

    /// partial_exists:记录在场即 true(乐观判定;真缺席由拒开降级兜底)。
    #[test]
    fn partial_exists_follows_record_face() {
        let home = tempfile::tempdir().unwrap();
        let d = dest(home.path());
        assert!(!d.partial_exists("a.bin"));
        put_record(
            &d,
            "a.bin",
            serde_json::json!({
                "rel": "a.bin", "size": 1,
                "sha256": format!("{:064}", 0), "chunk_count": 1,
                "bitmap": "00", "done": false, "final_rel": serde_json::Value::Null,
            }),
        );
        assert!(d.partial_exists("a.bin"));
    }

    /// resume 拒开 → 陈旧记录清除(降级全新重收的记录面)+ Err 上抛:
    /// 真实 resolver 的拒开路径(broadcast 收请求 → refuse),不触 fdpass。
    #[cfg(unix)]
    #[tokio::test]
    async fn resume_refusal_clears_stale_record() {
        let home = tempfile::tempdir().unwrap();
        let (resolver, mut evs) = resolver();
        let d = Arc::new(BridgeDestIo::new(home.path().to_path_buf(), resolver));
        let rel = "断点件.bin";
        put_record(
            &d,
            rel,
            serde_json::json!({
                "rel": rel, "size": 8,
                "sha256": format!("{:064}", 0), "chunk_count": 1,
                "bitmap": "00", "done": false, "final_rel": serde_json::Value::Null,
            }),
        );
        // open_writer 是懒 future——spawn 起跑(事件才会在等待期间发出),
        // 主线等事件到达即拒开(resolver 侧等价 0x02 无 fd)。
        let entry = FileEntry {
            rel_path: rel.to_string(),
            size: 8,
            sha256: vec![0u8; 32],
            chunks: vec![mtdss_proto::manifest::ChunkDesc {
                offset: 0,
                length: 8,
                sha256: vec![0u8; 32],
            }],
        };
        let writer = {
            let d = d.clone();
            tokio::spawn(async move { d.open_writer(&entry, true).await })
        };
        // 收到 fs.request_dest 事件即拒开
        let ev = tokio::time::timeout(std::time::Duration::from_secs(5), evs.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ev.method, "fs.request_dest");
        assert_eq!(ev.params["rel"].as_str(), Some(rel));
        assert_eq!(ev.params["resume"].as_bool(), Some(true));
        assert_eq!(ev.params["size"].as_u64(), Some(8));
        assert!(d.resolver.refuse(ev.params["request_id"].as_u64().unwrap(), "NOT_FOUND"));
        let err = writer.await.unwrap().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("目的地不可用"), "拒开词面: {msg}");
        assert!(msg.contains("NOT_FOUND"), "拒开原因透传(词面收口在 fdpass 分派侧,e2e 断言): {msg}");
        assert!(msg.contains("断点已清除"), "降级语义文案: {msg}");
        assert!(
            !d.record_file(rel).exists(),
            "拒开后陈旧记录必须被清除(重试轮将全新重收)"
        );
    }

    /// 哨兵常量锚(与 intake 单一真源对齐——本模块不另立字符串)。
    #[test]
    fn sentinel_value() {
        assert_eq!(INBOX_SENTINEL, "inbox");
    }
}
