//! 预览落盘缓存(V4-1 T5):office 整取等「全文件」预览面的磁盘缓存。
//!
//! 形态(spec §5「handle 私有落盘缓存」):缓存件键 = handle 串(注册表
//! 铸造的 32 hex,文件系统安全),落 `{home}/preview-tmp/{handle}.bin`
//! ——home 即桌面 `.mtdss` 数据目录(即 spec §5「`.mtdss` 下 preview-tmp
//! 」的落地路径),与 serve-tmp/inbox-staging 同级;**收件箱语义外,绝不
//! 触公开区**(serve_dir R8 终审 I-2 同款先例:iOS 暂存区注入
//! `{home}/.mtdss`,缓存若落注入命名空间内,App 挂载清扫/逐请求目录建删
//! 会摧毁在用缓存件,还与「预览不落收件箱」(spec §7)相抵——缓存目录
//! 必须与暂存区命名空间互斥;件名只收字母数字 handle,fail-closed)。
//!
//! 帽与淘汰(spec §5 用户裁定④):**无单文件上限**(天然封顶≈原文件量
//! 级;超帽单件照存不拒——在预览句柄受豁免保护,关 handle 后下一轮淘汰
//! 最先出清);唯一硬顶 = **全局磁盘 LRU 4 GiB**([`DISK_CACHE_CAP_BYTES`]
//! ,构造注入可调):总量超帽按「整取落盘序」淘汰最旧,**在预览的 handle
//! 跳过**(与 [`super::handles::HandleRegistry`] 联动判定「在预览」——
//! 最小接口 [`HandleRegistry::is_active`] 纯存在性查询,缓存不触注册表
//! 内部,注册表也不反向感知缓存;T15 段缓存键 `{handle}vseg{n}` 判定取
//! handle 前缀,裸 handle 键照原键)。淘汰序取落盘序的裁定:读面重用时序
//! 在本缓存不可观测——读(path_for 消费)只发生在 handle 活跃期,而活跃
//! 期条目全数豁免;关 handle 后无消费面,重用时序冻结,落盘序即唯一可
//! 观测的「最旧」。
//!
//! 记账:目录内 `ledger.json`(条目 `{handle, bytes}`,**向量序 = LRU
//! 序**,无需时间戳/序号)随每次 store/淘汰**原子写**
//! ([`crate::session::write_atomic`]:同目录临时件 fsync 后 rename)。
//! load 沿 tasks.json 模式:缺失 = 首次空表;解析失败/非 UTF-8 → 改名
//! `ledger.json.bad-<ts>` 留底后从空开始(缓存=可丢数据,拦启动不成比
//! 例);其他读失败 = warn 后按空继续(文件原位不动)。
//!
//! 启动初始化(挂 `DaemonState::load` = daemon 启动)三件事:建目录、整
//! 载记账、**对账清扫**——账外文件(崩溃残留 tmp、rename 后记账写败的
//! 孤儿)删除,账内文件缺席(外力删除)剔账,bytes 以盘上实长为准(外
//! 力手改面),账坏备份后从空 = 盘面随「账外」一并归零——记账与盘面在
//! 对账后一致,容量计费不失真。数据件**不 fsync**(缓存可丢面,rename
//! 就位即见;崩溃残留由下次启动对账清扫兜底),记账文件照 write_atomic
//! fsync(小件,无感)。
//!
//! 写失败活力面(tasks.json 口径的轻量版):ledger 落盘失败不拦内存权威
//! 面,每次 store/淘汰原样重试,恢复后下一次写全量补写;warn 逐次(写
//! 频 = 整取/淘汰粒度,极低,无限频记账面)。中断期进程死亡,孤儿文件
//! 由下次启动对账清扫。
//!
//! 纪律:store 同步阻塞(流读+落盘),调用方(T17/T18 office 整取)在
//! spawn_blocking 上驱动——blocking_fs 同款纪律;并发双 store 同 handle
//! = 各写唯一临时件(命名纪律沿 write_atomic 2026-11-16 根修),rename
//! 就位原子可见(消费面只见完整件),记账面同 handle 替换计费,末位胜。
//! 无后台任务(被动面:store/path_for/淘汰全同步),daemon 收口零钩子。

use std::io;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::handles::HandleRegistry;

/// 全局磁盘 LRU 帽(spec §5 用户裁定④「唯一硬顶=全局磁盘 LRU 4 GiB」;
/// 常量锚,改值须过 cache 测试)。
pub(crate) const DISK_CACHE_CAP_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// 缓存目录锚(`{home}/preview-tmp`;与 serve-tmp 同款「暂存区命名空间
/// 之外」裁定,见模块注)。
fn preview_tmp_dir(home: &Path) -> PathBuf {
    home.join("preview-tmp")
}

/// 记账文件名(目录内;条目向量序 = LRU 序)。
const LEDGER_FILE: &str = "ledger.json";

/// 缓存件后缀(`{handle}.bin`)。
const FILE_SUFFIX: &str = ".bin";

/// 记账条目(serde default:残缺记录前向兼容,沿 TaskRec 口径)。
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
#[serde(default)]
struct LedgerEntry {
    handle: String,
    bytes: u64,
}

/// 记账落盘形态(包装对象,沿 StoreDump 口径;新格式无旧形态,单形解析)。
#[derive(serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct LedgerDump {
    entries: Vec<LedgerEntry>,
}

/// 内存记账面(权威):向量序 = 整取落盘序(最旧在前)= 淘汰序;total
/// 与 entries 求和恒等(冗余存防逐次求和)。
#[derive(Default)]
struct CacheState {
    entries: Vec<LedgerEntry>,
    total: u64,
}

/// 临时件命名序号(并发唯一;write_atomic 2026-11-16 根修同款纪律)。
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// handle 键卫生(缓存件名 = handle + 后缀,目录面唯一接触点):只收字
/// 母数字串(注册表铸造的 32 hex 恒过;路径分隔/`..` 等一律拒——缓存件
/// 名不得越出 preview-tmp,「绝不触公开区」的 fail-closed 面)。
fn valid_handle(handle: &str) -> bool {
    !handle.is_empty() && handle.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// 预览落盘缓存(全 daemon 单份,随 DaemonState 常驻):全局磁盘 LRU 面。
pub(crate) struct PreviewDiskCache {
    dir: PathBuf,
    cap: u64,
    /// 「在预览」判定源(T4 注册表;is_active 纯查询,见模块注)。
    active: Arc<HandleRegistry>,
    inner: Mutex<CacheState>,
}

impl PreviewDiskCache {
    /// 生产构造:目录 = `{home}/preview-tmp`,帽 = 4 GiB;初始化(建目
    /// 录/整载/对账清扫)随构造完成(挂 DaemonState::load = daemon 启动)。
    pub(crate) fn new(home: &Path, active: Arc<HandleRegistry>) -> Self {
        Self::with_params(&preview_tmp_dir(home), DISK_CACHE_CAP_BYTES, active)
    }

    /// 参数注入构造(测试面:帽/目录可调——伪文件注入 = 小帽 + 小流,
    /// 免真写 4 GiB)。初始化三件事(见模块注):建目录、整载记账(含
    /// 损坏兜底)、对账清扫(剔缺账/校实长/扫孤儿)。
    fn with_params(dir: &Path, cap: u64, active: Arc<HandleRegistry>) -> Self {
        if let Err(e) = std::fs::create_dir_all(dir) {
            // 目录建不成 = 后续 store 诚实报错(不拦构造;构造无 Result)
            tracing::warn!(%e, dir = %dir.display(), "preview-tmp 目录创建失败");
        }
        let cache = Self {
            dir: dir.to_path_buf(),
            cap,
            active,
            inner: Mutex::new(CacheState::default()),
        };
        let entries = Self::load_ledger(dir);
        {
            let mut st = cache.inner.lock().unwrap();
            // 对账:缺文件剔账、实长校准(外力删除/手改面;bytes 以盘上
            // 实长为准,容量计费不失真)。向量的 LRU 序(最旧在前)照账
            // 保留;同 handle 重复条目(手改/外来文件)取末次出现,防淘
            // 汰面「删一次文件、余条目永远删不成」的僵死。
            let mut seen = std::collections::HashSet::new();
            for e in entries.into_iter().rev() {
                if !valid_handle(&e.handle) || !seen.insert(e.handle.clone()) {
                    continue;
                }
                match std::fs::metadata(cache.file_path(&e.handle)) {
                    Ok(m) => {
                        let real = m.len();
                        if real != e.bytes {
                            tracing::debug!(
                                handle = %e.handle,
                                ledger = e.bytes,
                                real,
                                "preview-tmp 记账与盘面不符,以实长为准"
                            );
                        }
                        st.entries.push(LedgerEntry {
                            handle: e.handle,
                            bytes: real,
                        });
                        st.total += real;
                    }
                    Err(_) => {
                        tracing::debug!(handle = %e.handle, "preview-tmp 记账条目盘上缺席,剔账")
                    }
                }
            }
            st.entries.reverse(); // 逆序收的(取末次出现)再翻回最旧在前
        }
        cache.sweep_orphans();
        cache
    }

    /// 整取落盘(stream 全量写缓存件,完成才可见):写唯一临时件 →
    /// rename 就位 → 记账(同 handle 替换计费)→ 超帽淘汰 → 原子落账。
    /// 返回缓存件路径(path_for 命中同路径)。流中失败 = 删临时件 + Err,
    /// 记账/盘面零痕。**不做单文件上限**(spec 用户裁定④):超帽单件照
    /// 存,受在预览豁免保护,关 handle 后淘汰轮出清。
    // 消费面:V4-1 T6 /jpeg 转码结果(loopback,Windows+heic 面)已挂;
    // T15(转码段)/T17/T18(office 整取)本波后续任务挂入。
    #[cfg_attr(not(all(windows, feature = "heic")), allow(dead_code))]
    pub(crate) fn store(&self, handle: &str, stream: &mut dyn Read) -> io::Result<PathBuf> {
        if !valid_handle(handle) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "handle 非法(仅限字母数字)",
            ));
        }
        let final_path = self.file_path(handle);
        let tmp_path = self.dir.join(format!(
            "{handle}.tmp-{}-{}",
            TMP_SEQ.fetch_add(1, Ordering::Relaxed),
            std::process::id()
        ));
        let written = {
            let mut f = std::fs::File::create(&tmp_path)?;
            match io::copy(stream, &mut f) {
                Ok(n) => n,
                Err(e) => {
                    drop(f);
                    let _ = std::fs::remove_file(&tmp_path);
                    return Err(e);
                }
            }
        };
        // rename 就位(Windows rename 不许目标在场,先摘旧——同 handle
        // 重整取 = 替换语义;数据件不 fsync,缓存可丢面,见模块注)
        if final_path.exists() {
            let _ = std::fs::remove_file(&final_path);
        }
        if let Err(e) = std::fs::rename(&tmp_path, &final_path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e);
        }
        // 记账 + 淘汰 + 原子落账(锁内全含:store 写频 = 整取粒度,极低;
        // 删旧件为快路径;path_for 只读短临界区,无争用面)
        {
            let mut st = self.inner.lock().unwrap();
            // 同 handle 替换计费(并发双 store 同 handle = 去重,末位胜)
            if let Some(prev) = st.entries.iter().position(|e| e.handle == handle) {
                st.total -= st.entries[prev].bytes;
                st.entries.remove(prev);
            }
            st.entries.push(LedgerEntry {
                handle: handle.to_string(),
                bytes: written,
            });
            st.total += written;
            self.evict_locked(&mut st, handle);
            self.flush_locked(&st);
        }
        Ok(final_path)
    }

    /// 缓存件路径(账内 Some,未整取 None;纯读不刷时序——见模块注「淘
    /// 汰序」裁定)。路径在场 ≠ 文件必在(外力删除面由消费方 open 失败
    /// 诚实呈现;下次启动对账剔账)。
    // 消费面:V4-1 T6 /jpeg 缓存命中(loopback,Windows+heic 面)已挂;
    // T15(HLS)/T17/T18(office 回环)后续任务挂入。
    #[cfg_attr(not(all(windows, feature = "heic")), allow(dead_code))]
    pub(crate) fn path_for(&self, handle: &str) -> Option<PathBuf> {
        if !valid_handle(handle) {
            return None;
        }
        let st = self.inner.lock().unwrap();
        st.entries
            .iter()
            .any(|e| e.handle == handle)
            .then(|| self.file_path(handle))
    }

    /// 整载记账(tasks.json 模式):缺失 = 首次空表;解析失败/非 UTF-8 =
    /// 改名 `ledger.json.bad-<ts>` 留底后从空开始;其他读失败 = warn 后按
    /// 空继续(文件原位不动)。缓存可丢数据,任何异常不拦构造(构造无
    /// Result)。
    fn load_ledger(dir: &Path) -> Vec<LedgerEntry> {
        let path = dir.join(LEDGER_FILE);
        match std::fs::read_to_string(&path) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) if err.kind() == io::ErrorKind::InvalidData => {
                backup_corrupt(dir, &path, "非 UTF-8 内容", &err);
            }
            Err(err) => {
                tracing::warn!(%err, path = %path.display(), "preview-tmp 记账读取失败,按空表继续");
            }
            Ok(s) => match serde_json::from_str::<LedgerDump>(&s) {
                Ok(dump) => return dump.entries,
                Err(err) => backup_corrupt(dir, &path, "解析失败", &err),
            },
        }
        Vec::new()
    }

    /// 对账清扫(启动初始化第三件事):目录内**非记账文件**(崩溃残留
    /// tmp、rename 后记账写败的孤儿、手投文件)一律删除——账是唯一权威,
    /// 账外即垃圾(账坏从空起 = 盘面一并归零,缓存可丢口径)。目录条目
    /// 与记账本件/备份件不动(备份件是损坏留底,与 tasks.json.bad- 同款
    /// 存续);删除失败不拦启动,warn 留痕(下次启动再试)。
    fn sweep_orphans(&self) {
        let tracked: std::collections::HashSet<String> = self
            .inner
            .lock()
            .unwrap()
            .entries
            .iter()
            .map(|e| format!("{}{FILE_SUFFIX}", e.handle))
            .collect();
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let bad_prefix = format!("{LEDGER_FILE}.bad-");
        for item in rd.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            if name == LEDGER_FILE || name.starts_with(&bad_prefix) || tracked.contains(&name) {
                continue;
            }
            // 目录条目不碰(fail-safe:清扫只及文件)
            if item.file_type().map(|t| t.is_dir()).unwrap_or(true) {
                continue;
            }
            if let Err(e) = std::fs::remove_file(item.path()) {
                tracing::warn!(%e, file = %name, "preview-tmp 账外文件清扫失败(下次启动再试)");
            }
        }
    }

    /// 淘汰(总量超帽):按向量序(最旧在前)挑豁免外受害者直到 ≤ 帽;
    /// 全数活跃(或只剩删除失败件)= 超帽容忍(在预览面优先,spec「不
    /// 影响在预览的 handle」)。受害者删除失败 = 留账续扫下一枚(单轮,
    /// 不循环重试——失败件下次淘汰轮再试)。`current` = 本轮刚整取的
    /// handle,一律不出清:生产流恒活跃豁免覆盖,此保险面闭合「store
    /// 返回即失效(返回路径指向不存在文件)」的荒谬契约(非活跃合成形
    /// 态的超帽单件照存保活)。
    fn evict_locked(&self, st: &mut CacheState, current: &str) {
        if st.total <= self.cap {
            return;
        }
        let mut victims: Vec<(usize, String, u64)> = Vec::new();
        let mut running = st.total;
        for (i, e) in st.entries.iter().enumerate() {
            if running <= self.cap {
                break;
            }
            if e.handle == current {
                continue; // 本 store 轮保护(见上注)
            }
            // 段键豁免取 handle 前缀:T15 段缓存键 = `{handle}vseg{n}`(注册
            // 表 handle = 32 hex 不含 "v",split 恒落在段后缀;/jpeg 面裸
            // handle 键 split 不中即原键判定,面不变)。整键对注册表恒
            // miss → 活跃视频的段件会被超帽轮当最旧逐出(存在性过滤+重拉
            // 兜底不红,但重转码/重拉带宽白烧,长片自压)。
            let base = e
                .handle
                .split_once("vseg")
                .map_or(e.handle.as_str(), |(prefix, _)| prefix);
            if self.active.is_active(base) {
                continue; // 在预览豁免
            }
            victims.push((i, e.handle.clone(), e.bytes));
            running -= e.bytes;
        }
        // 逆序摘账(索引不漂移);文件删除失败 = 计费回账,条目留任
        for (i, handle, bytes) in victims.into_iter().rev() {
            if let Err(e) = std::fs::remove_file(self.file_path(&handle)) {
                tracing::warn!(%e, handle = %handle, "preview-tmp 缓存件删除失败,留账下次再试");
                running += bytes;
                continue;
            }
            st.entries.remove(i);
        }
        st.total = running;
    }

    /// 记账 → ledger.json 原子写(write_atomic 沿 tasks.json 镜像纪律;
    /// 失败不停摆:内存权威面不受影响,下次 store/淘汰原样重试,恢复后
    /// 全量补写;warn 逐次——写频为整取/淘汰粒度,无刷屏面)。
    fn flush_locked(&self, st: &CacheState) {
        let dump = LedgerDump {
            entries: st.entries.clone(),
        };
        let body = match serde_json::to_string_pretty(&dump) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(%err, "preview-tmp 记账序列化失败,跳过本次落账");
                return;
            }
        };
        if let Err(err) = crate::session::write_atomic(&self.dir.join(LEDGER_FILE), &body) {
            tracing::warn!(%err, "preview-tmp 记账落盘失败(内存面不受影响,下次写重试)");
        }
    }

    fn file_path(&self, handle: &str) -> PathBuf {
        self.dir.join(format!("{handle}{FILE_SUFFIX}"))
    }

    /// 总字节(单测锚)。
    #[cfg(test)]
    fn total_bytes(&self) -> u64 {
        self.inner.lock().unwrap().total
    }

    /// 条目数(单测锚)。
    #[cfg(test)]
    fn len(&self) -> usize {
        self.inner.lock().unwrap().entries.len()
    }
}

/// 损坏兜底(解析失败/非 UTF-8 内容共用):改名 `ledger.json.bad-<ts>`
/// 留底(tasks.json 同款;毫秒戳防连坏多次互相覆盖;备份失败仅留痕)。
/// 备份件不被对账清扫误删(清扫 skip 面,见 [`PreviewDiskCache::
/// sweep_orphans`]),与 tasks.json.bad- 同款存续。
fn backup_corrupt(dir: &Path, path: &Path, what: &str, err: &dyn std::fmt::Display) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let bad = dir.join(format!("{LEDGER_FILE}.bad-{ts}"));
    match std::fs::rename(path, &bad) {
        Ok(()) => tracing::warn!(
            err = %err,
            bad = %bad.display(),
            "preview-tmp 记账损坏({what}),已备份后从空开始"
        ),
        Err(rerr) => tracing::warn!(
            err = %err,
            %rerr,
            path = %path.display(),
            "preview-tmp 记账损坏且备份失败,按空表继续"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::handles::CloseOutcome;
    use crate::preview::source::FileSource;
    use std::sync::Arc as StdArc;

    /// 内存假源(handles.rs 测试同款;注册表 open 需要现成源)。
    struct MockSource(u64);
    impl FileSource for MockSource {
        fn size(&self) -> u64 {
            self.0
        }
        fn read_at(&self, _off: u64, _buf: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    fn reg() -> StdArc<HandleRegistry> {
        StdArc::new(HandleRegistry::new())
    }

    /// 测试缓存(帽/目录注入;伪文件注入面 = 小帽 + 小流)。
    fn cache(dir: &Path, cap: u64, r: &StdArc<HandleRegistry>) -> PreviewDiskCache {
        PreviewDiskCache::with_params(dir, cap, StdArc::clone(r))
    }

    fn data(n: u8) -> Vec<u8> {
        vec![b'x'; n as usize]
    }

    fn dir_names(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    /// 常量锚(4 GiB;改值须过本断言)。
    #[test]
    fn cap_constant_anchor() {
        assert_eq!(DISK_CACHE_CAP_BYTES, 4 * 1024 * 1024 * 1024, "全局帽常量锚");
    }

    /// 整取往返:字节一致、path_for 即落位路径、未整取 None;同 handle
    /// 重整取 = 替换(单条目、计费随新件);目录零残留(无 tmp)。
    #[test]
    fn store_roundtrip_and_path_for() {
        let dir = tempfile::tempdir().unwrap();
        let r = reg();
        let c = cache(dir.path(), 1000, &r);
        let payload = data(37);
        let p = c
            .store("aaaa", &mut io::Cursor::new(payload.clone()))
            .unwrap();
        assert_eq!(p, c.path_for("aaaa").unwrap(), "path_for 即 store 返回路径");
        assert_eq!(std::fs::read(&p).unwrap(), payload, "缓存件字节一致");
        assert!(c.path_for("bbbb").is_none(), "未整取 = None");
        assert_eq!(c.total_bytes(), 37, "计费 = 实写字节");

        let payload2 = data(11);
        c.store("aaaa", &mut io::Cursor::new(payload2.clone()))
            .unwrap();
        assert_eq!(c.len(), 1, "同 handle 替换,不双计");
        assert_eq!(c.total_bytes(), 11, "替换计费随新件");
        assert_eq!(
            std::fs::read(c.path_for("aaaa").unwrap()).unwrap(),
            payload2
        );

        let names = dir_names(dir.path());
        assert!(
            names.iter().all(|n| n == "aaaa.bin" || n == LEDGER_FILE),
            "目录只含缓存件与记账(无 tmp 残留): {names:?}"
        );
    }

    /// handle 键卫生(「绝不触公开区」fail-closed 面):路径分隔/空串/
    /// 点号一律拒——store Err、path_for None、目录零写入。
    #[test]
    fn invalid_handle_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let r = reg();
        let c = cache(dir.path(), 100, &r);
        for bad in ["a/b", "..", "", "a.b", "a\\b", "h-1"] {
            let got = c.store(bad, &mut io::Cursor::new(data(5)));
            assert!(got.is_err(), "非法 handle {bad:?} 必拒");
            assert!(c.path_for(bad).is_none(), "非法 handle {bad:?} 查询 None");
        }
        assert!(
            dir_names(dir.path()).is_empty(),
            "非法 handle 零目录写入: {:?}",
            dir_names(dir.path())
        );
        let ok = c.store("abc123", &mut io::Cursor::new(data(5)));
        assert!(ok.is_ok(), "字母数字 handle 恒过(注册表 32 hex 同域)");
    }

    /// LRU 淘汰序(最旧先出):帽 100,h1(30)+h2(40) 共存;h3(50)
    /// 超帽 → h1 出;h4(50) → h2 出;幸存序 h3,h4;盘面同步出清;
    /// 淘汰随落账持久(reload 幸存面一致)。
    #[test]
    fn lru_evicts_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let r = reg();
        let c = cache(dir.path(), 100, &r);
        c.store("h1", &mut io::Cursor::new(data(30))).unwrap();
        c.store("h2", &mut io::Cursor::new(data(40))).unwrap();
        assert_eq!(c.total_bytes(), 70);
        assert!(
            c.path_for("h1").is_some() && c.path_for("h2").is_some(),
            "帽内共存"
        );
        c.store("h3", &mut io::Cursor::new(data(50))).unwrap();
        assert!(c.path_for("h1").is_none(), "最旧 h1 出清");
        assert!(c.path_for("h2").is_some() && c.path_for("h3").is_some());
        assert_eq!(c.total_bytes(), 90, "淘汰至帽内");
        assert!(!dir.path().join("h1.bin").exists(), "盘面同步出清");
        c.store("h4", &mut io::Cursor::new(data(50))).unwrap();
        assert!(c.path_for("h2").is_none(), "次旧 h2 出清");
        assert_eq!(c.total_bytes(), 100);

        let c2 = cache(dir.path(), 100, &r);
        assert_eq!(c2.len(), 2, "reload 条目数一致");
        assert_eq!(c2.total_bytes(), 100, "reload 计费一致");
        assert!(c2.path_for("h3").is_some() && c2.path_for("h4").is_some());
    }

    /// 在预览豁免:活跃 handle 的缓存件跳过淘汰(哪怕最旧);is_active
    /// 查询面顺钉(在场 true/关后 false);关 handle 归零后恢复可淘汰。
    /// 本 store 轮保护顺钉:hx(60,活跃最旧)与 h1(40)帽内共存后,h2
    /// (50)超帽轮——hx 活跃豁免、h2 本轮保护,唯一可淘汰面 = 次旧非
    /// 活跃 h1(「出清次旧保最旧活跃」即豁免的证词);关 handle 后 hx
    /// 不再豁免,下一轮即出清。
    #[test]
    fn in_preview_handles_skipped_by_eviction() {
        let dir = tempfile::tempdir().unwrap();
        let r = reg();
        let hx = r
            .open("d", "/x", Some(1), Some(10), StdArc::new(MockSource(10)))
            .unwrap();
        assert!(r.is_active(&hx), "在场 handle = 活跃(is_active 查询面)");
        let c = cache(dir.path(), 100, &r);
        c.store(&hx, &mut io::Cursor::new(data(60))).unwrap();
        c.store("h1", &mut io::Cursor::new(data(40))).unwrap(); // 100 = 帽
        assert_eq!(c.total_bytes(), 100);
        c.store("h2", &mut io::Cursor::new(data(50))).unwrap(); // 150 > 帽
        assert!(c.path_for(&hx).is_some(), "在预览豁免:最旧但活跃,不出清");
        assert!(c.path_for("h1").is_none(), "次旧非活跃 h1 出清");
        assert!(
            c.path_for("h2").is_some(),
            "h2 本 store 轮保护(且豁免面只剩活跃 hx)"
        );
        assert_eq!(c.total_bytes(), 110, "无可淘汰面 = 超帽容忍");
        assert_eq!(r.close(&hx), CloseOutcome::Removed, "关 handle 归零");
        assert!(!r.is_active(&hx), "关后非活跃");
        c.store("h3", &mut io::Cursor::new(data(10))).unwrap(); // 120 > 帽
        assert!(c.path_for(&hx).is_none(), "关 handle 后不再豁免,出清");
        assert!(c.path_for("h2").is_some() && c.path_for("h3").is_some());
        assert_eq!(c.total_bytes(), 60);
    }

    /// 段键豁免(T15 面回归):seg 缓存键 `{handle}vseg{n}` 的在预览豁免
    /// 取 handle 前缀判定——整键对注册表恒 miss,活跃视频的段件会被超帽
    /// 轮当最旧逐出(长片自压面)。活跃 handle 的 `vseg3` 段件跳过淘汰;
    /// 空闲 handle 的段件照常可淘汰;关 handle 后段件豁免即失(下一轮出
    /// 清)。裸 handle 键(/jpeg 面)的豁免既有测试另钉,面不变。
    #[test]
    fn segment_cache_keys_exempt_via_handle_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let r = reg();
        let hx = r
            .open("d", "/x", Some(1), Some(10), StdArc::new(MockSource(10)))
            .unwrap();
        let seg_active = format!("{hx}vseg3");
        let c = cache(dir.path(), 100, &r);
        c.store(&seg_active, &mut io::Cursor::new(data(60))).unwrap();
        c.store("idleh0vseg0", &mut io::Cursor::new(data(40))).unwrap(); // 100 = 帽
        c.store("h2", &mut io::Cursor::new(data(50))).unwrap(); // 150 > 帽
        assert!(
            c.path_for(&seg_active).is_some(),
            "活跃 handle 段件豁免(前缀判定,最旧也不出清)"
        );
        assert!(
            c.path_for("idleh0vseg0").is_none(),
            "空闲 handle 段件照常出清(前缀不在注册表)"
        );
        assert!(
            c.path_for("h2").is_some(),
            "h2 本 store 轮保护(豁免面只剩活跃段件)"
        );
        assert_eq!(c.total_bytes(), 110, "无可淘汰面 = 超帽容忍");
        assert_eq!(r.close(&hx), CloseOutcome::Removed, "关 handle 归零");
        c.store("h3", &mut io::Cursor::new(data(10))).unwrap(); // 120 > 帽
        assert!(c.path_for(&seg_active).is_none(), "关后段件不再豁免,出清");
        assert!(c.path_for("h2").is_some() && c.path_for("h3").is_some());
        assert_eq!(c.total_bytes(), 60);
    }

    /// 容量计费(伪文件注入):小帽注入下,手投盘面文件 + 手写记账
    /// (伪文件,免真写 4 GiB)→ 装载即按账计费;超帽淘汰按账序;计费
    /// 恒 = 实写字节;reload 计费一致(落账持久)。
    #[test]
    fn capacity_billing_with_injected_pseudo_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.bin"), data(25)).unwrap();
        std::fs::write(dir.path().join("b.bin"), data(35)).unwrap();
        let ledger = r#"{"entries":[{"handle":"a","bytes":25},{"handle":"b","bytes":35}]}"#;
        std::fs::write(dir.path().join(LEDGER_FILE), ledger).unwrap();
        let r = reg();
        let c = cache(dir.path(), 40, &r);
        assert_eq!(c.total_bytes(), 60, "装载即按账计费");
        assert_eq!(c.len(), 2);
        c.store("c", &mut io::Cursor::new(data(20))).unwrap(); // 80 > 40
        assert!(
            c.path_for("a").is_none() && c.path_for("b").is_none(),
            "a、b 依序出清"
        );
        assert_eq!(c.total_bytes(), 20);
        c.store("d", &mut io::Cursor::new(data(19))).unwrap(); // 39 ≤ 40
        assert_eq!(c.total_bytes(), 39, "计费恒 = 实写字节");
        let c2 = cache(dir.path(), 40, &r);
        assert_eq!(c2.total_bytes(), 39, "reload 计费一致(落账持久)");
    }

    /// 损坏兜底(tasks.json 模式):ledger.json 解析失败 → `.bad-<ts>`
    /// 备份留底后从空开始;失账缓存件随对账清扫归零;备份件本身不被清扫
    /// 误删;后续 store 正常。
    #[test]
    fn corrupt_ledger_backs_up_and_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(LEDGER_FILE), "{not json").unwrap();
        std::fs::write(dir.path().join("old.bin"), data(5)).unwrap();
        let r = reg();
        let c = cache(dir.path(), 100, &r);
        assert_eq!(c.len(), 0, "损坏后从空开始");
        let names = dir_names(dir.path());
        let bads: Vec<&String> = names
            .iter()
            .filter(|n| n.starts_with(&format!("{LEDGER_FILE}.bad-")))
            .collect();
        assert_eq!(bads.len(), 1, "恰好一枚 .bad-<ts> 备份: {names:?}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(bads[0])).unwrap(),
            "{not json",
            "备份原样留底"
        );
        assert!(
            !names.contains(&"old.bin".to_string()),
            "失账缓存件随对账清扫归零: {names:?}"
        );
        c.store("n", &mut io::Cursor::new(data(3))).unwrap();
        assert!(c.path_for("n").is_some(), "损坏兜底后 store 正常");
    }

    /// 对账清扫(启动初始化):账外文件(孤儿件/崩溃残留 tmp)删除,账
    /// 内文件保留,记账缺席条目剔账(ghost),实长校准(账 9 盘 5 → 按
    /// 5 计费);目录条目与记账本件不碰。
    #[test]
    fn init_reconciles_and_sweeps_orphans() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keep.bin"), data(5)).unwrap();
        std::fs::write(dir.path().join("orphan.bin"), data(99)).unwrap();
        std::fs::write(dir.path().join("h3.tmp-7-4242"), data(77)).unwrap();
        std::fs::create_dir_all(dir.path().join("子目录")).unwrap();
        let ledger =
            r#"{"entries":[{"handle":"ghost","bytes":12345},{"handle":"keep","bytes":9}]}"#;
        std::fs::write(dir.path().join(LEDGER_FILE), ledger).unwrap();
        let r = reg();
        let c = cache(dir.path(), 1000, &r);
        assert_eq!(c.len(), 1, "ghost 剔账");
        assert_eq!(c.total_bytes(), 5, "实长校准(账 9 → 盘 5)");
        assert!(c.path_for("keep").is_some());
        assert!(c.path_for("ghost").is_none());
        assert!(!dir.path().join("orphan.bin").exists(), "孤儿删除");
        assert!(!dir.path().join("h3.tmp-7-4242").exists(), "tmp 残留删除");
        assert!(dir.path().join("子目录").is_dir(), "目录条目不碰");
        assert!(dir.path().join(LEDGER_FILE).exists(), "记账本件不碰");
    }

    /// 流中失败:临时件删除、零记账、path_for None、失败后照常(活力面)。
    #[test]
    fn midstream_failure_leaves_no_trace() {
        struct Boom;
        impl Read for Boom {
            fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("流中断"))
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let r = reg();
        let c = cache(dir.path(), 100, &r);
        let got = c.store("h", &mut Boom);
        assert!(got.is_err(), "流中失败上抛");
        assert!(c.path_for("h").is_none(), "零记账");
        assert_eq!(c.len(), 0);
        assert!(
            dir_names(dir.path()).is_empty(),
            "目录零残留(tmp 已删、零落账): {:?}",
            dir_names(dir.path())
        );
        c.store("h", &mut io::Cursor::new(data(4))).unwrap();
        assert!(c.path_for("h").is_some(), "失败后 store 照常");
    }

    /// 超帽单件照存(无单文件上限,spec 用户裁定④):帽 50,单件 80 →
    /// Ok 且计费 80(超帽容忍——活跃期豁免,无可淘汰面);关 handle 后
    /// 下一发触发其出清(不再豁免)。拒绝存储反而违裁定,回归红线。
    /// **本 store 轮保护**:非活跃超帽单件(合成形态,生产恒活跃)同样
    /// 照存保活——store 返回的路径不得在返回即失效(自清出「返回指向
    /// 不存在文件」的荒谬契约)。
    #[test]
    fn single_file_over_cap_is_stored_not_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let r = reg();
        let hx = r
            .open("d", "/x", Some(1), Some(80), StdArc::new(MockSource(80)))
            .unwrap();
        let c = cache(dir.path(), 50, &r);
        let big: Vec<u8> = (0..80u8).collect();
        let p = c.store(&hx, &mut io::Cursor::new(big.clone())).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), big, "超帽单件完整落盘");
        assert_eq!(c.total_bytes(), 80, "计费照实(超帽容忍)");
        r.close(&hx);
        c.store("s", &mut io::Cursor::new(data(1))).unwrap();
        assert!(c.path_for(&hx).is_none(), "超帽件在可淘汰时出清");
        assert_eq!(c.total_bytes(), 1);

        // 本 store 轮保护(非活跃合成形态):大件照存、路径即刻可用;
        // 同轮可淘汰面 = s(1B,非活跃非本轮),照常出清
        let big2: Vec<u8> = (100..180u8).collect();
        let p2 = c.store("huge", &mut io::Cursor::new(big2.clone())).unwrap();
        assert_eq!(std::fs::read(&p2).unwrap(), big2, "非活跃超帽单件完整落盘");
        assert!(
            c.path_for("huge").is_some(),
            "store 返回路径即刻有效(本轮不自清)"
        );
        assert!(c.path_for("s").is_none(), "同轮 s 出清(LRU 照走)");
        assert_eq!(c.total_bytes(), 80, "计费随出清收敛(超帽部分容忍)");
    }

    /// daemon 集成点(最小接线):DaemonState::load 即完成缓存目录初始化
    /// ——`{home}/preview-tmp` 在场,句柄随 state 可取且与 home 同源落盘
    /// (消费面 T6/T15/T17/T18 后续挂入;本测试钉「初始化挂启动」)。
    #[test]
    fn daemon_state_initializes_cache_dir() {
        let (state, home) = test_state_with_home();
        let dir = home.path().join("preview-tmp");
        assert!(dir.is_dir(), "启动即建缓存目录");
        let c = state.preview_cache();
        assert!(c.path_for("无").is_none(), "空缓存查询 None");
        let payload = data(3);
        c.store("i", &mut io::Cursor::new(payload)).unwrap();
        assert!(dir.join("i.bin").exists(), "state 侧缓存落 home 同源目录");
    }

    /// 测试态(独立临时 home;preview.rs tests 同款装配面单点;保活
    /// TempDir 供目录断言;预置会话非必需——load 无登录面要求)。
    fn test_state_with_home() -> (StdArc<crate::state::DaemonState>, tempfile::TempDir) {
        let home = tempfile::tempdir().unwrap();
        let (events, _) = tokio::sync::broadcast::channel(16);
        #[cfg(windows)]
        let update = StdArc::new(crate::update::UpdateService::new(
            crate::update::UpdateConfig::default(),
            events.clone(),
            tokio::sync::watch::channel(false).0,
        ));
        #[cfg(not(windows))]
        let update = ();
        let state = StdArc::new(
            crate::state::DaemonState::load(
                home.path(),
                home.path().join("inbox"),
                None,
                crate::inbox_dest::staging_dir(home.path()),
                false,
                false,
                Default::default(),
                Default::default(),
                crate::ctrl::ctrl_default_tuning(),
                events,
                update,
            )
            .unwrap(),
        );
        (state, home)
    }
}
