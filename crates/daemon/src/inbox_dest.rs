//! 收件私有暂存目的地(P4a):Android 收件箱的 [`DestIo`] 实现——
//! [`FsDestIo`] 的**薄包装**。
//!
//! 磁盘面(spec §3.1/§3.5)全在 daemon 私有暂存区 `{home}/inbox-staging`:
//! - 记录与 `.part`:`{home}/inbox-staging/.mtdss/`(intake 统一 schema/
//!   键空间/原子写,**与桌面 FsDestIo 同一布局**——写/续传/对账全部原样
//!   复用,零行为差;键空间天然隔离于桌面默认收件目录);
//! - 终件:`{home}/inbox-staging/<final_rel>`(finalize 原子改名落位,
//!   冲突改名 `name (n).ext` 由 FsDestIo 的 `unique_target` 天然携带);
//! - `published` 标记:P4a 增设的记录字段(发布成功后打上,intake 的
//!   `FileRecord` 是宽松 serde——未知字段加载侧忽略,桌面记录不含它),
//!   daemon 侧经 JSON 直读写(本模块),core 不动。
//!
//! 唯一覆写:**`final_state`**(spec §6.2)——记录门(done + published)
//! 给出候选 size 后,由**在场查询**收口(`fs.query_dest` → fdpass
//! `0x04`,App 查 MediaStore 真实行);**绝不 stat 暂存路径**——staged
//! 但未发布的件不得 Skip(恰是本设计要杀的假完成类),F7 失败方向铁律
//! (超时/无应答/不在场 → None 保守传输)原样承袭。桌面对账零改动(覆写
//! 只挂本收件实现)。
//!
//! 发布协调器(jobs.rs)的暂存面也在本模块:`publish_backlog`(轮落定
//! 路径 → 待发布集)、`settle_published`(删暂存终件 + 记录打标)、
//! `cleanup_inbox`(transfer.clear 的残留回收)、`sweep_legacy_records`
//! (v2-P3 旧 `inbox-records` 布局的启动清扫)。
//!
//! cfg 无关(全部本地 fs + 无 fd 的查询/发布通道;收件箱**轮的路由**在
//! flow.rs 受 cfg(unix) 门,Windows 桌面行为逐字节不变)——host 双平台
//! 单测可直构。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::future::BoxFuture;
use mtdss_core::transport::intake::{
    cleanup_rels, record_key, DestIo, FsDestIo, INTAKE_DIR,
};
use mtdss_proto::manifest::FileEntry;

use crate::jobs::DestQueries;

/// 私有暂存区目录名(home 下;`.mtdss`/`tasks.json` 等 home 级键空间
/// 并列,互不触碰)。
pub(crate) const STAGING_DIR: &str = "inbox-staging";

/// v2-P3 旧收件桥的记录目录名(退役布局,启动清扫目标,见
/// [`sweep_legacy_records`])。
const LEGACY_RECORDS_DIR: &str = "inbox-records";

/// 记录 JSON 的 published 标记字段名(P4a 增设;桌面记录缺席该字段,
/// 读侧按 false 处理)。
const PUBLISHED_FIELD: &str = "published";

/// 私有暂存区路径(`{home}/inbox-staging`)。
pub(crate) fn staging_dir(home: &Path) -> PathBuf {
    home.join(STAGING_DIR)
}

/// 收件箱暂存记录路径(`{home}/inbox-staging/.mtdss/<sha256(rel)>.json`)。
fn record_file(home: &Path, rel: &str) -> PathBuf {
    staging_dir(home)
        .join(INTAKE_DIR)
        .join(format!("{}.json", record_key(rel)))
}

/// 收件箱目的地:FsDestIo(`{home}/inbox-staging`)薄包装——写/续传/
/// 终件/对账全部委托(`records_dir`/`partial_exists`/`discard_partial`/
/// `prepare_overwrite`/`free_bytes`/`open_writer`/`commit` 零变化),唯
/// `final_state` 覆写为「记录门(done+published)→ 0x04 查询」。每接收轮
/// 一个实例(resident loop 哨兵路由处构造);跨轮断点续传靠记录面。
pub(crate) struct InboxDestIo {
    inner: FsDestIo,
    queries: Arc<DestQueries>,
}

impl InboxDestIo {
    pub(crate) fn new(home: PathBuf, queries: Arc<DestQueries>) -> Self {
        Self {
            inner: FsDestIo::new(home.join(STAGING_DIR)),
            queries,
        }
    }
}

impl DestIo for InboxDestIo {
    fn records_dir(&self) -> PathBuf {
        self.inner.records_dir()
    }

    fn final_state(&self, final_rel: &str) -> Option<u64> {
        // 记录门(§6.2 双层的前层):done + published + final_rel 吻合才有
        // 候选 size。按 final_rel 反查记录(收件暂存的冲突改名使
        // final_rel ≠ rel,记录键 = sha256(rel),无从正查——扫目录匹配,
        // unique_target 保证 final_rel 实际唯一)。读失败/字段不符/未
        // published 一律 None;**绝不 stat 暂存路径**(staged-but-
        // unpublished 正是要判死的那类假完成)。
        let size = published_size(&self.records_dir(), final_rel)?;
        // 在场层(双层后层,F7):记录门只证明「已发布过」,不证明「现在
        // 还在」——问 App(0x04,MediaStore 真实行)。同步签名下的等待桥
        // 与 BridgeDestIo 旧同款:intake 对账在阻塞线程池调用,oneshot
        // 有界等待经 Handle::block_on 落地;无 runtime 上下文即保守 None。
        let queries = self.queries.clone();
        let rel = final_rel.to_string();
        let presence = tokio::runtime::Handle::try_current()
            .ok()
            .and_then(|h| h.block_on(async move { queries.query(&rel, size).await }))?;
        (presence.exists && presence.size == size).then_some(size)
    }

    fn partial_exists(&self, rel: &str) -> bool {
        self.inner.partial_exists(rel)
    }

    fn discard_partial(&self, rel: &str) {
        self.inner.discard_partial(rel)
    }

    fn prepare_overwrite(&self, rel: &str) {
        self.inner.prepare_overwrite(rel)
    }

    fn free_bytes(&self) -> u64 {
        self.inner.free_bytes()
    }

    fn open_writer<'a>(
        &'a self,
        entry: &'a FileEntry,
        resume: bool,
    ) -> BoxFuture<'a, anyhow::Result<tokio::fs::File>> {
        self.inner.open_writer(entry, resume)
    }

    fn commit<'a>(
        &'a self,
        rel: &'a str,
        part: &'a mut tokio::fs::File,
    ) -> BoxFuture<'a, anyhow::Result<mtdss_core::transport::intake::Committed>> {
        self.inner.commit(rel, part)
    }
}

// ---------- 发布协调器的暂存面(jobs.rs 消费) ----------

/// 发布项(协调器载荷):一轮落定后的单个待发布暂存件。
#[derive(Clone, Debug)]
pub(crate) struct PublishItem {
    /// 清单 rel(记录键定位/published 打标用)。
    pub rel: String,
    /// 发布名 = 记录 final_rel(发布名与 0x04 键同源,§6.4;冲突改名
    /// `name (n).ext` 天然携带)。
    pub final_rel: String,
    /// 暂存终件绝对路径(fs.publish 的 staging_path;发布 ok 后删除)。
    pub path: PathBuf,
    /// 字节数(fs.publish 的 size,记录面读出)。
    pub size: u64,
}

/// 轮落定路径 → 待发布集:扫暂存记录目录,`final_rel` 命中落定集且
/// **done 未 published** 的记录入集。守恒方向:读失败/畸形/已 published
/// 一律静默跳过(宁可少发布,不误删他人素材);目录缺席 = 空集(退化
/// 面由调用方沿旧收口)。阻塞 fs 面(协调器侧 spawn_blocking 调用)。
pub(crate) fn publish_backlog(home: &Path, landed: &[PathBuf]) -> Vec<PublishItem> {
    let staging = staging_dir(home);
    // 落定路径 → 可移植 final_rel(剥暂存前缀 + 平台分隔符归一 '/')
    let want: std::collections::HashSet<String> = landed
        .iter()
        .filter_map(|p| {
            p.strip_prefix(&staging)
                .ok()
                .map(|r| r.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/"))
        })
        .collect();
    if want.is_empty() {
        return Vec::new();
    }
    let records = staging.join(INTAKE_DIR);
    let mut out: Vec<PublishItem> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in std::fs::read_dir(&records).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".json") || name.ends_with(".json.tmp") {
            continue; // 记录 tmp/无关件
        }
        let Ok(bytes) = std::fs::read(entry.path()) else {
            continue; // 读失败:宁少发布不误判
        };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue; // 畸形记录:同上
        };
        let Some(final_rel) = v["final_rel"].as_str() else {
            continue; // 未完成(final_rel null)或畸形
        };
        if !want.contains(final_rel) || !seen.insert(final_rel.to_string()) {
            continue; // 非本轮落定件 / final_rel 重复(取首见)
        }
        if v["done"].as_bool() != Some(true) || v[PUBLISHED_FIELD].as_bool() == Some(true) {
            continue; // 未完成 / 已发布(幂等重入防御)
        }
        let (Some(rel), Some(size)) = (v["rel"].as_str(), v["size"].as_u64()) else {
            continue; // 身份/尺寸面不完整
        };
        out.push(PublishItem {
            rel: rel.to_string(),
            final_rel: final_rel.to_string(),
            path: staging.join(final_rel.replace('/', std::path::MAIN_SEPARATOR_STR)),
            size,
        });
    }
    out
}

/// 发布成功的本地收尾(协调器逐件调用,阻塞 fs 面):**先删暂存终件,
/// 后记录打 published**(顺序钉死,§6.2——两步间崩溃最坏走重收重发布,
/// App 侧撞名由发布面 policy 收口;反序则崩溃留下「记录已 published 而
/// 暂存件泄明文」的 F4 同款孤儿)。打标为 tmp+rename 原子写。尽力而为,
/// 失败仅留痕(App 的 ok 应答是发布事实的终审)。
pub(crate) fn settle_published(home: &Path, item: &PublishItem) {
    if let Err(e) = std::fs::remove_file(&item.path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(%e, path = %item.path.display(), "删除暂存终件失败(尽力而为)");
        }
    }
    if !mark_record_published(home, &item.rel) {
        tracing::warn!(
            rel = %item.rel,
            "记录 published 打标失败(重发将对账重收,不损数据)"
        );
    }
}

/// 记录打 published:读改写 + tmp+rename 原子替换(字段面与 intake
/// FileRecord 一致,published 为增设字段)。false = 记录缺席/畸形/写失败。
fn mark_record_published(home: &Path, rel: &str) -> bool {
    let path = record_file(home, rel);
    let Ok(bytes) = std::fs::read(&path) else {
        return false;
    };
    let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    v[PUBLISHED_FIELD] = serde_json::Value::Bool(true);
    let tmp = path.with_extension("json.tmp");
    let write = serde_json::to_vec(&v)
        .map_err(|e| e.to_string())
        .and_then(|b| std::fs::write(&tmp, b).map_err(|e| e.to_string()));
    if let Err(e) = write {
        tracing::warn!(%e, path = %tmp.display(), "published 打标 tmp 写失败");
        return false;
    }
    match std::fs::rename(&tmp, &path) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(%e, path = %path.display(), "published 打标 rename 失败");
            false
        }
    }
}

/// 记录门查询:final_rel → 已发布记录的候选 size(扫目录匹配
/// final_rel && done && published;首见即返)。None = 无门(未完成/
/// 未发布/缺席/畸形)。
fn published_size(records_dir: &Path, final_rel: &str) -> Option<u64> {
    for entry in std::fs::read_dir(records_dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".json") || name.ends_with(".json.tmp") {
            continue;
        }
        let Ok(bytes) = std::fs::read(entry.path()) else {
            continue;
        };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if v["final_rel"].as_str() != Some(final_rel) {
            continue;
        }
        if v["done"].as_bool() != Some(true) || v[PUBLISHED_FIELD].as_bool() != Some(true) {
            continue; // 门未过:不问 App,更不 stat 暂存
        }
        if let Some(size) = v["size"].as_u64() {
            return Some(size);
        }
    }
    None
}

/// 收件箱清除的本地残留回收(`transfer.clear`,jobs.rs 消费;阻塞 fs
/// 面):记录与 `.part` 经 `cleanup_rels` 的身份终审语义(暂存区就是
/// FsDestIo 布局,经 [`InboxDestIo`] 定位);已落定(done)记录的暂存
/// **终件**按记录 final_rel 一并回收——旧桥 provider 侧无此面,P4a 起
/// 暂存在本地(不回收即孤儿)。返回完成清理的 rel 数(cleanup_rels
/// 口径)。
pub(crate) fn cleanup_inbox(home: &Path, rels: &[String], queries: &Arc<DestQueries>) -> u32 {
    // 先记终件面(cleanup_rels 会删记录,读面在删前)
    let staging = staging_dir(home);
    let mut finals: Vec<PathBuf> = Vec::new();
    for rel in rels {
        let Some(v) = read_record(home, rel) else {
            continue;
        };
        if v["done"].as_bool() == Some(true) {
            if let Some(fr) = v["final_rel"].as_str() {
                finals.push(staging.join(fr.replace('/', std::path::MAIN_SEPARATOR_STR)));
            }
        }
    }
    let dest = InboxDestIo::new(home.to_path_buf(), queries.clone());
    let cleaned = cleanup_rels(&dest, rels);
    for p in finals {
        let _ = std::fs::remove_file(p); // 尽力而为:记录已删,残留件下次清除幂等跳过
    }
    cleaned
}

/// 读一条暂存记录(JSON);缺席/畸形 → None。
fn read_record(home: &Path, rel: &str) -> Option<serde_json::Value> {
    let raw = std::fs::read_to_string(record_file(home, rel)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// v2-P3 旧收件桥记录布局的启动清扫(`{home}/inbox-records`,Daemon::
/// start 调用):直发未上架无存量用户,残留只在测试机——**不做迁移**
/// (spec §2 非目标),装新版自然清。尽力而为:失败仅留痕不拦启动。
pub(crate) fn sweep_legacy_records(home: &Path) {
    let legacy = home.join(LEGACY_RECORDS_DIR);
    if !legacy.exists() {
        return;
    }
    match std::fs::remove_dir_all(&legacy) {
        Ok(()) => tracing::info!(dir = %legacy.display(), "已清扫 v2-P3 旧收件桥记录布局(无迁移,spec §2 非目标)"),
        Err(e) => tracing::warn!(%e, dir = %legacy.display(), "旧收件桥记录布局清扫失败(忽略,不拦启动)"),
    }
}

// ---------- 单元测试(host 双平台:全部本地 fs + 无 fd 的查询面) ----------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{DestQueries, EV_PUBLISH};
    use mtdss_ipc::EventFrame;
    use std::sync::Mutex;

    /// 测试装配:真实 DestQueries + 测试自持的事件接收端(broadcast 驱动
    /// 0x04 应答;不触 fdpass)。
    fn queries() -> (Arc<DestQueries>, tokio::sync::broadcast::Receiver<EventFrame>) {
        let (events, rx) = tokio::sync::broadcast::channel(16);
        (Arc::new(DestQueries::new(events)), rx)
    }

    fn dest(home: &Path) -> InboxDestIo {
        let (q, _rx) = queries();
        InboxDestIo::new(home.to_path_buf(), q)
    }

    /// 落一份暂存记录(字段面与 intake FileRecord 一致;published 可选)。
    fn put_record(home: &Path, rel: &str, final_rel: Option<&str>, published: bool) {
        let dir = staging_dir(home).join(INTAKE_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let v = serde_json::json!({
            "rel": rel,
            "size": 42,
            "sha256": format!("{:064}", 0),
            "chunk_count": 1,
            "bitmap": "01",
            "done": final_rel.is_some(),
            "final_rel": final_rel,
            "published": published,
        });
        std::fs::write(record_file(home, rel), serde_json::to_vec(&v).unwrap()).unwrap();
    }

    /// 布局锚:暂存区 = {home}/inbox-staging,记录 = 其下 .mtdss
    /// (FsDestIo 同款,桌面键空间隔离)。
    #[test]
    fn staging_layout_is_home_scoped() {
        let home = tempfile::tempdir().unwrap();
        let d = dest(home.path());
        assert_eq!(staging_dir(home.path()), home.path().join("inbox-staging"));
        assert_eq!(d.records_dir(), home.path().join("inbox-staging").join(".mtdss"));
    }

    /// final_state 记录门(§6.2,评审焦点 1 的本体):done+published → 0x04
    /// 在场查询收口(在场且 size 吻合 → Some;不在场 → None);**done 未
    /// published → None(不问,更不 stat 暂存——暂存终件就是在场也绝不
    /// Skip)**;final_rel 不符/记录缺席 → None。
    #[tokio::test]
    async fn final_state_gates_on_published_record_then_queries_presence() {
        let home = tempfile::tempdir().unwrap();
        let (q, evs) = queries();
        let d = InboxDestIo::new(home.path().to_path_buf(), q.clone());
        put_record(home.path(), "docs/a.bin", Some("docs/a.bin"), true); // 门过
        put_record(home.path(), "docs/staged.bin", Some("docs/staged.bin"), false); // staged 未发布
        put_record(home.path(), "docs/del.bin", Some("docs/del.bin"), true); // 门过,App 答已删
        put_record(home.path(), "docs/b.bin", Some("docs/改名.bin"), true); // final_rel 不符该键
        // 「守卫」本体:staged 未发布件在暂存区**真实在场**——记录门未过,
        // 不得因其在场而 Some(绝不 stat 暂存路径)
        std::fs::create_dir_all(staging_dir(home.path()).join("docs")).unwrap();
        std::fs::write(staging_dir(home.path()).join("docs/staged.bin"), b"staged").unwrap();
        // 伪 App:fs.query_dest 到达即应答(docs/a.bin 与 docs/改名.bin
        // 在场;docs/del.bin 已删)
        let answerer = tokio::spawn(async move {
            let mut evs = evs;
            while let Ok(ev) = evs.recv().await {
                if ev.method != "fs.query_dest" {
                    continue;
                }
                let id = ev.params["request_id"].as_u64().unwrap();
                let rel = ev.params["rel"].as_str().unwrap().to_string();
                let size = ev.params["size"].as_u64().unwrap();
                let present = matches!(rel.as_str(), "docs/a.bin" | "docs/改名.bin");
                assert!(
                    q.answer(id, present, if present { size } else { 0 }),
                    "在场查询应答必须被受理: {rel}"
                );
            }
        });
        // 生产调用面:对账在阻塞线程池上调 final_state(同步签名)。
        let got = tokio::task::spawn_blocking(move || {
            (
                d.final_state("docs/a.bin"),
                d.final_state("docs/staged.bin"),
                d.final_state("docs/del.bin"),
                d.final_state("docs/改名.bin"),
                d.final_state("docs/missing.bin"),
            )
        })
        .await
        .unwrap();
        answerer.abort();
        assert_eq!(got.0, Some(42), "门过 + 在场 + size 吻合 → Some");
        assert_eq!(
            got.1, None,
            "staged 未发布:暂存件在场也必须 None(绝不 stat 暂存)"
        );
        assert_eq!(got.2, None, "门过但 App 答已删(F7)→ None");
        assert_eq!(got.3, Some(42), "final_rel 命中他键记录(冲突改名反查)→ Some");
        assert_eq!(got.4, None, "记录缺席 → None");
    }

    /// publish_backlog:done 未 published + final_rel 命中落定集 → 入集;
    /// published/未完成/非本轮落定件跳过;settle_published 后终件删除 +
    /// 记录打标(再扫为空)。
    #[test]
    fn backlog_and_settle_roundtrip() {
        let home = tempfile::tempdir().unwrap();
        put_record(home.path(), "a.bin", Some("a.bin"), false);
        put_record(home.path(), "b.bin", Some("b (1).bin"), false); // 冲突改名携带
        put_record(home.path(), "pub.bin", Some("pub.bin"), true); // 已发布
        put_record(home.path(), "part.bin", None, false); // 未完成
        let staging = staging_dir(home.path());
        for (f, c) in [("a.bin", b"a"), ("b (1).bin", b"b"), ("pub.bin", b"p")] {
            std::fs::write(staging.join(f), c).unwrap();
        }
        let landed = vec![staging.join("a.bin"), staging.join("b (1).bin")];
        let items = publish_backlog(home.path(), &landed);
        let mut rels: Vec<&str> = items.iter().map(|i| i.rel.as_str()).collect();
        rels.sort();
        assert_eq!(rels, vec!["a.bin", "b.bin"], "恰两件待发布: {items:?}");
        let a = items.iter().find(|i| i.rel == "a.bin").unwrap();
        assert_eq!(a.final_rel, "a.bin");
        assert_eq!(a.path, staging.join("a.bin"));
        assert_eq!(a.size, 42);
        // settle:终件删除 + 记录打 published
        for it in &items {
            settle_published(home.path(), it);
        }
        assert!(!staging.join("a.bin").exists(), "发布后终件删除");
        assert!(!staging.join("b (1).bin").exists(), "发布后终件删除(改名件)");
        for rel in ["a.bin", "b.bin"] {
            let v = read_record(home.path(), rel).unwrap();
            assert_eq!(v["published"].as_bool(), Some(true), "{rel} 打标");
            assert_eq!(v["done"].as_bool(), Some(true), "{rel} 记录保留");
        }
        // 幂等重扫:已发布不入集
        assert!(
            publish_backlog(home.path(), &landed).is_empty(),
            "published 后重扫为空"
        );
    }

    /// cleanup_inbox:身份吻合的记录 + `.part` + done 终件三件全回收;
    /// 未请求 rel 的素材不动。
    #[test]
    fn cleanup_inbox_removes_record_part_and_final() {
        use mtdss_core::transport::intake::{part_path, record_path};
        let home = tempfile::tempdir().unwrap();
        let staging = staging_dir(home.path());
        let rel = "docs/a.bin";
        put_record(home.path(), rel, Some("docs/a.bin"), false);
        std::fs::write(part_path(&staging, rel), b"part").unwrap();
        std::fs::create_dir_all(staging.join("docs")).unwrap();
        std::fs::write(staging.join("docs").join("a.bin"), b"final").unwrap();
        // 旁人素材:不被请求
        put_record(home.path(), "other.bin", Some("other.bin"), false);
        std::fs::write(staging.join("other.bin"), b"o").unwrap();

        let (q, _rx) = queries();
        let cleaned = cleanup_inbox(home.path(), &[rel.to_string()], &q);
        assert_eq!(cleaned, 1, "恰一条记录被清");
        assert!(!record_path(&staging, rel).exists(), "记录删除");
        assert!(!part_path(&staging, rel).exists(), ".part 删除");
        assert!(!staging.join("docs").join("a.bin").exists(), "done 终件回收");
        assert!(staging.join("other.bin").exists(), "未请求素材不动");
        assert!(
            record_path(&staging, "other.bin").exists(),
            "未请求记录不动"
        );
    }

    /// 启动清扫:v2-P3 旧 inbox-records 布局连根删除;缺席时为 no-op。
    #[test]
    fn sweep_legacy_records_removes_old_layout() {
        let home = tempfile::tempdir().unwrap();
        let legacy = home.path().join("inbox-records");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("junk.json"), b"{}").unwrap();
        sweep_legacy_records(home.path());
        assert!(!legacy.exists(), "旧布局必须被清扫");
        sweep_legacy_records(home.path()); // 幂等:缺席 no-op 不 panic
    }

    /// DestQueries 统一面(host 直驱,不触 fdpass):0x04 与 0x05 共用同
    /// 一 id 计数器(§6.5——交替应答 id 连续单调,无两套空间);发布应答
    /// ok:false 的 error 文案透传。超时/无应答面由 e2e(注入 1500ms)覆盖。
    #[tokio::test]
    async fn dest_queries_shared_counter_and_publish_faces() {
        let (q, mut evs) = queries();
        let seen: Arc<Mutex<Vec<(u64, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let qq = q.clone();
        let answerer = tokio::spawn(async move {
            let mut n = 0u64;
            while let Ok(ev) = evs.recv().await {
                let id = ev.params["request_id"].as_u64().unwrap();
                log.lock().unwrap().push((id, ev.method.clone()));
                match ev.method.as_str() {
                    "fs.query_dest" => {
                        assert!(qq.answer(id, true, 42), "0x04 应答受理");
                    }
                    EV_PUBLISH => {
                        n += 1;
                        let ok = n == 1;
                        assert!(
                            qq.answer_publish(id, ok, if ok { None } else { Some("注入失败") }),
                            "0x05 应答受理"
                        );
                    }
                    _ => {}
                }
            }
        });
        // 混发 0x04/0x05:id 必须来自同一单调面(全序无重号)
        let presence = q.query("a.bin", 42).await.expect("查询在场");
        assert!(presence.exists && presence.size == 42);
        let ok_path = std::path::PathBuf::from("/tmp/x");
        assert!(
            q.publish("a.bin", &ok_path, 42).await.is_ok(),
            "首次发布 ok"
        );
        let err = q.publish("b.bin", &ok_path, 42).await.unwrap_err();
        assert!(
            err.contains("App 发布失败") && err.contains("注入失败"),
            "失败文案透传: {err}"
        );
        let ids: Vec<u64> = seen.lock().unwrap().iter().map(|(i, _)| *i).collect();
        assert_eq!(ids, vec![0, 1, 2], "统一 id 计数器:交替请求全序单调");
        answerer.abort();
    }
}
