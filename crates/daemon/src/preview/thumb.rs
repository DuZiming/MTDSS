//! 源端缩略图引擎面(V4-1 T7):生成+缓存,平台无关。
//!
//! 引擎面(brief 钉死):[`thumb`]`(path, mtime, size, w) -> Vec<u8>`——
//! JPEG 字节;**失败/无后端 = 空字节**(发起端回退类型图标,spec §4「失败
//! 回退=类型图标」;不回错误帧——错误帧会打破「缩略图缺位只是降级」的
//! 客户端语义)。mtime/size 缺省 0(proto「不可得」口径)照常入键。
//!
//! 源端缓存(spec §4):key = `hash(path|mtime|size|w)`([`cache_key`],
//! SHA-256 hex,文件系统安全),独立缓存目录 `{home}/thumb-tmp`,全局磁盘
//! LRU [`THUMB_CACHE_CAP_BYTES`] = 512 MiB。房法沿 T5 [`super::cache`]
//! (ledger.json 记账、原子写、启动对账清扫、损坏备份、孤儿清扫),两处
//! 刻意差异:①**无在预览豁免**——条目键是内容 hash 非 handle,没有「在
//! 预览」的可判定面,淘汰一律可出清(缩略图重生成廉价,丢命中无损正确
//! 性);②**LRU 序 = 真读序**——T5 的读面只发生在 handle 活跃期(豁免
//! 覆盖,时序不可观测),缩略图的读面(二次浏览零重生成)时时发生且可
//! 观测,命中即刷新时序([`ThumbCache::load`]),「常看的不出清」才有意义。
//!
//! 空结果**不入缓存**(失败面:无后端/解码失败可能是暂态——装了新解码器
//! /文件补全;负缓存会把暂态失败钉成永久缺图,而重生成失败面本就廉价,
//! 亚秒级 shell 探测)。命中缺失的 ghost 条目(外力删文件)不剔账,下一
//! 次 store 同键替换自愈(T5 启动对账兜底归零面照旧)。
//!
//! 生成:[`generate`] 平台分发——Windows = [`super::thumb_win`] Shell
//! IShellItemImageFactory(COM,系统原生,覆盖图片/视频抽帧/PDF 首页/
//! HEIC,零第三方解码;`SIIGBF_THUMBNAILONLY` 保证「提不出缩略图」如实
//! 失败,绝不拿图标充数);非 Windows = 空字节(V4-3 波补,期间客户端
//! 回退类型图标)。生成全程 spawn_blocking(COM 与 fs 都是阻塞调用,T6
//! 同款纪律)。测试计数器 [`GEN_CALLS`] 供「缓存命中零重生成」断言。
//!
//! 纪律:无单飞去重(同键并发 miss 双生成,末位胜替换计费——生成廉价,
//! 注册簿式去重的复杂度不抵收益);store 同步阻塞,调用方在 spawn_blocking
//! 之外的本函数已收口(生成在 blocking 池,store 在其后的异步面同步做,
//! 量级 KB 级单文件,无挂 Worker 面)。

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use mtdss_proto::ctrl::ThumbRequest;

/// 全局磁盘 LRU 帽(spec §4「源端缓存目录 LRU 上限 512 MiB」;常量锚,
/// 改值须过 thumb 测试)。
pub(crate) const THUMB_CACHE_CAP_BYTES: u64 = 512 * 1024 * 1024;

/// 请求宽钳制上界(远端/查询串可携任意 `w`——巨 `w` 的位图分配是内存炸
/// 弹面;平铺视图三档 ≤256px,1024 留十倍余量)。下界 1(0 无意义)。
pub(crate) const THUMB_MAX_WIDTH: u32 = 1024;

/// 桥回 JPEG 读入帽(防 App 侧 bug/异常把巨文件当缩略图回——1024px
/// JPEG 常态 ≤ 数百 KB,16 MiB 留 50 倍余量;超帽 = 桥失败面,落回退)。
const THUMB_BRIDGE_JPEG_CAP_BYTES: u64 = 16 * 1024 * 1024;

/// 缓存目录锚(`{home}/thumb-tmp`;与 preview-tmp 同级同款「暂存区命名
/// 空间之外」裁定,见 T5 cache.rs 模块注)。
fn thumb_tmp_dir(home: &Path) -> PathBuf {
    home.join("thumb-tmp")
}

/// App 生成件临时目录锚(`{home}/thumb-serve`;与 App 侧
/// MtdssEngine.thumbServeDir 同布局,两侧不得漂移)。与 [`thumb_tmp_dir`]
/// (引擎缓存目录)**刻意分立**:App 侧挂载清扫整目录清空,若同目录会把
/// 引擎 LRU 缓存一并抹掉;目录本体归 App(建目录/清扫),引擎只读+逐件
/// unlink(serve-tmp/photo-serve 三面同构)。
pub(crate) fn thumb_serve_dir(home: &Path) -> PathBuf {
    home.join("thumb-serve")
}

/// 回传路径是否落在 App 缩略图临时目录内(serve::path_under_serve_dir
/// 同款防御:App 侧 bug 不得诱使引擎读/删前缀外文件。词法面:规范化后
/// 整组件前缀;非 UTF-8/非法形态一律不在 fail-closed)。
pub(crate) fn path_under_thumb_serve_dir(home: &Path, p: &Path) -> bool {
    match (
        crate::roots::normalize_lexical(&thumb_serve_dir(home)),
        crate::roots::normalize_lexical(p),
    ) {
        (Ok(dir), Ok(path)) => path.starts_with(&dir),
        _ => false,
    }
}

/// 记账文件名(目录内;条目向量序 = LRU 序,T5 同名同款)。
const LEDGER_FILE: &str = "ledger.json";

/// 缓存件后缀(内容恒 JPEG)。
const FILE_SUFFIX: &str = ".jpg";

/// 记账条目(`{key, bytes}`;serde default 前向兼容,沿 T5 LedgerEntry
/// 口径;向量序 = LRU 序)。
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
#[serde(default)]
struct LedgerEntry {
    key: String,
    bytes: u64,
}

/// 记账落盘形态(沿 T5 LedgerDump 单形解析)。
#[derive(serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct LedgerDump {
    entries: Vec<LedgerEntry>,
}

/// 内存记账面(权威):向量序 = LRU 序(最旧在前 = 淘汰序);total 与
/// entries 求和恒等(冗余存防逐次求和)。
#[derive(Default)]
struct CacheState {
    entries: Vec<LedgerEntry>,
    total: u64,
}

/// 缓存键卫生(T5 valid_handle 同款 fail-closed):键 = 64 hex(纯字母
/// 数字),路径分隔/点号/空串一律拒——缓存件名不得越出 thumb-tmp。
fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// 源端缩略图磁盘缓存(全 daemon 单份,随 DaemonState 常驻):512 MiB
/// LRU 面(语义见模块注)。
pub(crate) struct ThumbCache {
    dir: PathBuf,
    cap: u64,
    inner: Mutex<CacheState>,
}

impl ThumbCache {
    /// 生产构造:目录 = `{home}/thumb-tmp`,帽 = 512 MiB;初始化(建目
    /// 录/整载/对账清扫)随构造完成(挂 DaemonState::load = daemon 启动,
    /// T5 同款)。
    pub(crate) fn new(home: &Path) -> Self {
        Self::with_params(&thumb_tmp_dir(home), THUMB_CACHE_CAP_BYTES)
    }

    /// 参数注入构造(测试面:帽/目录可调)。初始化三件事沿 T5:建目录、
    /// 整载记账(含损坏备份兜底)、对账清扫(剔缺账/校实长/扫孤儿)。
    fn with_params(dir: &Path, cap: u64) -> Self {
        if let Err(e) = std::fs::create_dir_all(dir) {
            // 目录建不成 = 后续 store 诚实报错(不拦构造;T5 同款口径)
            tracing::warn!(%e, dir = %dir.display(), "thumb-tmp 目录创建失败");
        }
        let cache = Self {
            dir: dir.to_path_buf(),
            cap,
            inner: Mutex::new(CacheState::default()),
        };
        let entries = Self::load_ledger(dir);
        {
            let mut st = cache.inner.lock().unwrap();
            // 对账(T5 同款):缺文件剔账、实长校准(外力删除/手改面);
            // LRU 序照账保留;同键重复条目取末次出现,防淘汰僵死。
            let mut seen = std::collections::HashSet::new();
            for e in entries.into_iter().rev() {
                if !valid_key(&e.key) || !seen.insert(e.key.clone()) {
                    continue;
                }
                match std::fs::metadata(cache.file_path(&e.key)) {
                    Ok(m) => {
                        let real = m.len();
                        if real != e.bytes {
                            tracing::debug!(
                                key = %e.key,
                                ledger = e.bytes,
                                real,
                                "thumb-tmp 记账与盘面不符,以实长为准"
                            );
                        }
                        st.entries.push(LedgerEntry {
                            key: e.key,
                            bytes: real,
                        });
                        st.total += real;
                    }
                    Err(_) => {
                        tracing::debug!(key = %e.key, "thumb-tmp 记账条目盘上缺席,剔账")
                    }
                }
            }
            st.entries.reverse(); // 逆序收的(取末次出现)再翻回最旧在前
        }
        cache.sweep_orphans();
        cache
    }

    /// 取缓存(命中 = JPEG 字节 + LRU 时序刷新到最新):账内 key 才查;
    /// 文件被外力删除 = None(ghost 条目不剔账,同键 store 自愈,见模块
    /// 注)。**命中刷新读序**是本缓存与 T5 的刻意差异(模块注②)。
    pub(crate) fn load(&self, key: &str) -> Option<Vec<u8>> {
        if !valid_key(key) {
            return None;
        }
        let mut st = self.inner.lock().unwrap();
        let idx = st.entries.iter().position(|e| e.key == key)?;
        let bytes = std::fs::read(self.file_path(key)).ok()?;
        // 命中:移到向量尾(最新);O(n) 键数小,无感。
        let e = st.entries.remove(idx);
        st.entries.push(e);
        Some(bytes)
    }

    /// 存缓存(非空 JPEG;调用方保证):写唯一临时件 → rename 就位 →
    /// 记账(同键替换移尾)→ 超帽淘汰 → 原子落账。失败 = 删临时件 +
    /// false(缓存可丢面,best-effort;T5 流中失败同口径)。
    pub(crate) fn store(&self, key: &str, jpeg: &[u8]) -> bool {
        if !valid_key(key) || jpeg.is_empty() {
            return false;
        }
        use std::io::Write as _;
        let tmp_path = self.dir.join(format!(
            "{key}.tmp-{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            std::process::id()
        ));
        let written = (|| -> io::Result<u64> {
            let mut f = std::fs::File::create(&tmp_path)?;
            f.write_all(jpeg)?;
            Ok(jpeg.len() as u64)
        })();
        let written = match written {
            Ok(n) => n,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp_path);
                tracing::warn!(%e, key, "thumb 缓存临时件写失败(本次不缓存)");
                return false;
            }
        };
        let final_path = self.file_path(key);
        // rename 就位(Windows rename 不许目标在场,先摘旧;数据件不
        // fsync——缓存可丢面,T5 同款纪律)
        if final_path.exists() {
            let _ = std::fs::remove_file(&final_path);
        }
        if let Err(e) = std::fs::rename(&tmp_path, &final_path) {
            let _ = std::fs::remove_file(&tmp_path);
            tracing::warn!(%e, key, "thumb 缓存件就位失败(本次不缓存)");
            return false;
        }
        {
            let mut st = self.inner.lock().unwrap();
            // 同键替换计费(并发双 store = 去重,末位胜)+ 移尾(最新)
            if let Some(prev) = st.entries.iter().position(|e| e.key == key) {
                st.total -= st.entries[prev].bytes;
                st.entries.remove(prev);
            }
            st.entries.push(LedgerEntry {
                key: key.to_string(),
                bytes: written,
            });
            st.total += written;
            self.evict_locked(&mut st);
            self.flush_locked(&st);
        }
        true
    }

    /// 淘汰(总量超帽):按向量序(最旧在前)出清至 ≤ 帽。无豁免面(模块
    /// 注①);删除失败 = 留账续扫下一枚(单轮,T5 同款)。
    fn evict_locked(&self, st: &mut CacheState) {
        if st.total <= self.cap {
            return;
        }
        let mut victims: Vec<(usize, String, u64)> = Vec::new();
        let mut running = st.total;
        for (i, e) in st.entries.iter().enumerate() {
            if running <= self.cap {
                break;
            }
            victims.push((i, e.key.clone(), e.bytes));
            running -= e.bytes;
        }
        // 逆序摘账(索引不漂移);删除失败 = 计费回账,条目留任
        for (i, key, bytes) in victims.into_iter().rev() {
            if let Err(e) = std::fs::remove_file(self.file_path(&key)) {
                tracing::warn!(%e, key = %key, "thumb 缓存件删除失败,留账下次再试");
                running += bytes;
                continue;
            }
            st.entries.remove(i);
        }
        st.total = running;
    }

    /// 整载记账(T5 同款):缺失 = 首次空表;解析失败/非 UTF-8 = 改名
    /// `.bad-<ts>` 留底后从空开始;其他读失败 = warn 后按空继续。
    fn load_ledger(dir: &Path) -> Vec<LedgerEntry> {
        let path = dir.join(LEDGER_FILE);
        match std::fs::read_to_string(&path) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) if err.kind() == io::ErrorKind::InvalidData => {
                backup_corrupt(dir, "非 UTF-8 内容", &err);
            }
            Err(err) => {
                tracing::warn!(%err, path = %path.display(), "thumb-tmp 记账读取失败,按空表继续");
            }
            Ok(s) => match serde_json::from_str::<LedgerDump>(&s) {
                Ok(dump) => return dump.entries,
                Err(err) => backup_corrupt(dir, "解析失败", &err),
            },
        }
        Vec::new()
    }

    /// 对账清扫(T5 同款):账外文件一律删除(账是唯一权威;目录条目与
    /// 记账本件/备份件不动;删除失败不拦启动,下次启动再试)。
    fn sweep_orphans(&self) {
        let tracked: std::collections::HashSet<String> = self
            .inner
            .lock()
            .unwrap()
            .entries
            .iter()
            .map(|e| {
                let k = e.key.clone();
                format!("{k}{FILE_SUFFIX}")
            })
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
            if item.file_type().map(|t| t.is_dir()).unwrap_or(true) {
                continue;
            }
            if let Err(e) = std::fs::remove_file(item.path()) {
                tracing::warn!(%e, file = %name, "thumb-tmp 账外文件清扫失败(下次启动再试)");
            }
        }
    }

    /// 记账 → ledger.json 原子写(T5 同款;失败不停摆,下次 store 重试)。
    fn flush_locked(&self, st: &CacheState) {
        let dump = LedgerDump {
            entries: st.entries.clone(),
        };
        let body = match serde_json::to_string_pretty(&dump) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(%err, "thumb-tmp 记账序列化失败,跳过本次落账");
                return;
            }
        };
        if let Err(err) = crate::session::write_atomic(&self.dir.join(LEDGER_FILE), &body) {
            tracing::warn!(%err, "thumb-tmp 记账落盘失败(内存面不受影响,下次写重试)");
        }
    }

    fn file_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}{FILE_SUFFIX}"))
    }

    /// 条目键序(单测锚;向量序 = LRU 序,最旧在前)。
    #[cfg(test)]
    fn key_order(&self) -> Vec<String> {
        self.inner
            .lock()
            .unwrap()
            .entries
            .iter()
            .map(|e| e.key.clone())
            .collect()
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

/// 损坏兜底(T5 同款):改名 `ledger.json.bad-<ts>` 留底后从空开始;
/// 毫秒戳防连坏多次互相覆盖;备份件不被清扫误删(bad- 前缀 skip 面)。
fn backup_corrupt(dir: &Path, what: &str, err: &dyn std::fmt::Display) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let bad = dir.join(format!("{LEDGER_FILE}.bad-{ts}"));
    match std::fs::rename(dir.join(LEDGER_FILE), &bad) {
        Ok(()) => tracing::warn!(
            err = %err,
            bad = %bad.display(),
            "thumb-tmp 记账损坏({what}),已备份后从空开始"
        ),
        Err(rerr) => tracing::warn!(
            err = %err,
            %rerr,
            "thumb-tmp 记账损坏且备份失败,按空表继续"
        ),
    }
}

/// 缓存键(spec §4 钉死口径 `hash(path|mtime|size|w)`):SHA-256,域分隔
/// 以「竖线 + 各分量的定长编码」承载——路径字节流(变长)夹在定长界标间
/// 仍无歧义(`a|1` 与 `a` 拼 `|1` 的构造歧义由逐分量 update + 显式分隔
/// 字节闭合:mtime/size/w 各 8/8/4 字节定长,路径段以分隔字节终止)。
pub(crate) fn cache_key(path: &str, mtime: i64, size: u64, w: u32) -> String {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(b"thumb-v1");
    h.update(b"|");
    h.update(path.as_bytes());
    h.update(b"|");
    h.update(mtime.to_le_bytes());
    h.update(size.to_le_bytes());
    h.update(w.to_le_bytes());
    hex::encode(h.finalize())
}

/// 生成计数(仅测试面:「缓存命中零重生成」断言锚,heic DECODE_CALLS
/// 同款;generate 是全引擎唯一触碰点)。
#[cfg(test)]
static GEN_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// 生成计数用例的串行锁(并行测试下增量不失真;heic TEST_SERIAL 同款,
/// tokio Mutex——回环口用例跨 await 持锁)。
#[cfg(all(test, windows))]
pub(crate) static TEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 平台生成分发(COM/fs 阻塞面,调用方 spawn_blocking):成功 = JPEG
/// 字节;失败/无后端 = 空(调用方跳过缓存,见模块注「空结果不入缓存」)。
fn generate(path: &Path, w: u32) -> Vec<u8> {
    #[cfg(test)]
    GEN_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    // cfg 双臂互斥编译,单臂在场的尾表达式即返回值(无 return 面)。
    #[cfg(windows)]
    {
        super::thumb_win::shell_thumbnail(path, w).unwrap_or_default()
    }
    #[cfg(not(windows))]
    {
        let _ = (path, w);
        Vec::new()
    }
}

/// 缩略图引擎面(brief 钉死 `(path,mtime,size,w)->Vec<u8>`):缓存命中
/// 零重生成直出;miss 经 spawn_blocking 平台生成,非空结果入缓存。失败
/// /无后端 = 空字节(发起端回退类型图标)。键与生成源同路(本机路径直
/// 驱臂);桥 vpath 臂走 [`thumb_keyed`]。
pub(crate) async fn thumb(
    cache: &ThumbCache,
    path: &str,
    mtime: i64,
    size: u64,
    w: u32,
) -> Vec<u8> {
    thumb_keyed(cache, path, path, mtime, size, w).await
}

/// 键/源分离引擎面(评审 Important #1 回归面,桥 vpath 臂消费):缓存
/// 键按 `key_path`(vpath 原词——跨请求稳定标识)铸,生成从 `gen_path`
/// 读(桥臂 = 导出临时件,**每次导出路径不同**)。键若随临时件漂移,同
/// 一 vpath 每次都是 miss + 新键落账,512 MiB LRU 被永不命中的唯一键
/// 灌满——两参必须由调用方保持「同键同内容」语义。w 钳制与键铸在此单
/// 点,两臂共享。
pub(crate) async fn thumb_keyed(
    cache: &ThumbCache,
    key_path: &str,
    gen_path: &str,
    mtime: i64,
    size: u64,
    w: u32,
) -> Vec<u8> {
    let w = w.clamp(1, THUMB_MAX_WIDTH);
    let key = cache_key(key_path, mtime, size, w);
    if let Some(bytes) = cache.load(&key) {
        return bytes;
    }
    let path_buf = PathBuf::from(gen_path);
    // 生成收阻塞池(COM/fs 阻塞;JoinError = 池收尾,映失败空字节)
    let jpeg = tokio::task::spawn_blocking(move || generate(&path_buf, w))
        .await
        .unwrap_or_default();
    if !jpeg.is_empty() {
        cache.store(&key, &jpeg);
    }
    jpeg
}

/// `ThumbRequest` 的 ctrl 应答(分发缝 [`super::handle_ctrl_request_reply`]
/// 的守卫已过):三臂路由(读面路由同构,read_range_reply 镜像)——
/// 服务桥 vpath = **缩略图桥先行**(fs.request_thumb → App 0x0B JPEG,键
/// 按 vpath 原词铸,命中零广播;V4-1 T9)→ Err/空落**既有导出件回退**
/// (T7 轨,勿重写);照片 vpath = 桥先行 → Err/空落**照片导出件回退**
/// (stat_with_id → photo.export,T8 导轨);其余 = 本机路径直驱。
/// → 引擎面 → `ThumbResponse`。
///
/// **恒成功帧**(brief「失败 → ThumbResponse 空字节」):路由失败/生成
/// 失败/桥不可用一律 `ok=true` + 空 jpeg,不回错误帧——发起端把空字节映类
/// 型图标,错误帧反而要逐端适配降级面。**不做声明 size 核对**(与
/// read_range_reply 的 NOT_FOUND 口径刻意不同):缩略图缓存键含 size,陈
/// 旧声明只会生成「当前文件的新键下新图」,自愈不污染;拒发反而把轻微
/// 陈旧劣化成「类型图标」。
pub(crate) async fn thumb_reply(
    state: std::sync::Arc<crate::state::DaemonState>,
    t: ThumbRequest,
) -> mtdss_core::transport::ctrl::CtrlReply {
    let w = t.width;
    let size = if t.size > 0 { t.size as u64 } else { 0 };
    let jpeg = if let Some(hit) = state.guard().bridge_route(&t.path) {
        // 服务桥 vpath(V4-1 T9 缩略图桥先行):fs.request_thumb → App
        // 0x0B JPEG 临时件(缓存命中零广播,键按 vpath 原词铸——评审
        // Important #1 同口径);桥不可用/拒/超时/空字节 = None → 落**既
        // 有导出件回退**(T7 轨,勿重写):导出临时件落盘后以其真实路径
        // 喂后端生成(COM 按路径开文件;导出件随 ExportFileSource drop
        // 清理,生成期活到引擎面返回)。
        match thumb_via_bridge(&state, &t.path, t.mtime, size, w).await {
            Some(jpeg) => jpeg,
            None => match super::route_bridge_export(&state, &hit.root, &hit.sub).await {
                Ok((_temp, _export)) => {
                    // _export 手柄活到引擎面返回:临时件随其 Drop 清理(fail-
                    // closed 前缀内才删);缩略图生成以 _temp 真实落盘路径进行。
                    let temp_s = _temp.to_string_lossy().into_owned();
                    thumb_keyed(
                        &state.preview_thumb_cache(),
                        &t.path,
                        &temp_s,
                        t.mtime,
                        size,
                        w,
                    )
                    .await
                }
                Err(_e) => {
                    tracing::debug!("thumb 桥导出不可达,空字节回退");
                    Vec::new()
                }
            },
        }
    } else if crate::photo::photo_vpath(&t.path).is_some() {
        // 照片 vpath(V4-1 T9 新臂,泛化前落 fs 直驱恒空):桥先行 → Err/
        // 空落**照片导出件回退**(T8 导轨:photo.stat_with_id 单查 photo_id
        // 导出凭据 → photo.export 导出件 → 引擎面从导出件生成;回传路径
        // 前缀检查 fail-closed)。
        match thumb_via_bridge(&state, &t.path, t.mtime, size, w).await {
            Some(jpeg) => jpeg,
            None => thumb_photo_export_fallback(&state, &t.path, t.mtime, size, w).await,
        }
    } else {
        thumb(&state.preview_thumb_cache(), &t.path, t.mtime, size, w).await
    };
    mtdss_core::transport::ctrl::CtrlReply::Frame(mtdss_proto::ctrl::CtrlResponse {
        version: mtdss_proto::CTRL_VERSION,
        ok: true,
        body: Some(mtdss_proto::ctrl::ctrl_response::Body::Thumb(
            mtdss_proto::ctrl::ThumbResponse { jpeg },
        )),
    })
}

/// 缩略图桥先行臂(V4-1 T9;服务桥 vpath 与照片 vpath 两臂共用):
/// `fs.request_thumb` → App 0x0B JPEG 临时件 → 读字节(前缀校验
/// fail-closed + 读入帽)→ 入源端缓存(键按 vpath 原词铸,[`thumb_keyed`]
/// 的「同键同内容」语义:App 生成件路径每次不同,绝不入键)→ 删临时件
/// → 字节。缓存命中零广播。None = 桥不可用/超时/拒/空字节/前缀越界/
/// 读败(调用方落既有回退;空字节不入缓存,T7 语义——App「生成失败但
/// ok」的空件与拒答同面收口)。读收阻塞池(fs 阻塞,T5/T7 同款纪律)。
async fn thumb_via_bridge(
    state: &std::sync::Arc<crate::state::DaemonState>,
    vpath: &str,
    mtime: i64,
    size: u64,
    width: u32,
) -> Option<Vec<u8>> {
    let w = width.clamp(1, THUMB_MAX_WIDTH);
    let cache = state.preview_thumb_cache();
    let key = cache_key(vpath, mtime, size, w);
    if let Some(bytes) = cache.load(&key) {
        return Some(bytes);
    }
    let temp = state.jobs().thumb_bridge().open(vpath, w).await.ok()?;
    if !path_under_thumb_serve_dir(&state.home(), &temp) {
        // 前缀外拒读**且拒删**(cleanup_serve_temps 同款防御:前缀外路径
        // 不触引擎的删除面;App 清扫自收)
        tracing::warn!(temp = %temp.display(), "thumb 桥回传路径越出缩略图临时目录,拒读");
        return None;
    }
    // 读字节收阻塞池;帽超/读败 = 桥失败面(删临时件,None 落回退)
    let temp_for_read = temp.clone();
    let read = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
        let len = std::fs::metadata(&temp_for_read)?.len();
        if len > THUMB_BRIDGE_JPEG_CAP_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("thumb 回件 {len} 字节,超 {THUMB_BRIDGE_JPEG_CAP_BYTES} 帽"),
            ));
        }
        std::fs::read(&temp_for_read)
    })
    .await
    .ok()
    .and_then(|r| r.ok());
    // 引擎侧清理义务(读毕/读败都删;App 挂载清扫兜底)
    let _ = std::fs::remove_file(&temp);
    let jpeg = read?;
    if jpeg.is_empty() {
        return None;
    }
    cache.store(&key, &jpeg);
    Some(jpeg)
}

/// 照片 vpath 的缩略图回退(V4-1 T9;T8 导轨复用,勿重写):photo.stat_with_id
/// 单查(photo_id 导出凭据)→ photo.export 导出件(`{home}/photo-serve/`,
/// 前缀检查 fail-closed)→ [`thumb_keyed`](vpath 键/导出件源分离)。photo
/// 桥缺席/单查失败/无凭据/导出失败/越界 = 空字节(降级语义,与路由词面
/// 口径一致)。导出件手柄活到引擎面返回(Drop 清理临时件;route_photo 同形)。
async fn thumb_photo_export_fallback(
    state: &std::sync::Arc<crate::state::DaemonState>,
    vpath: &str,
    mtime: i64,
    size: u64,
    w: u32,
) -> Vec<u8> {
    let Some(photo) = state.photo() else {
        tracing::debug!("thumb 照片回退:照片桥未启用,空字节");
        return Vec::new();
    };
    let Ok((_data, Some(photo_id))) = photo.stat_with_id(vpath).await else {
        tracing::debug!("thumb 照片回退:单查不可达/无导出凭据,空字节");
        return Vec::new();
    };
    let Ok(temp) = photo.export(&photo_id).await else {
        tracing::debug!("thumb 照片回退:导出失败,空字节");
        return Vec::new();
    };
    if !crate::photo::path_under_serve_dir(&state.home(), &temp) {
        tracing::warn!(temp = %temp.display(), "thumb 照片回传路径越出照片临时目录,拒读");
        return Vec::new();
    }
    let Ok(export) = super::source::ExportFileSource::open(
        temp.clone(),
        Some(crate::photo::serve_dir(&state.home())),
    ) else {
        tracing::debug!("thumb 照片导出件打开失败,空字节");
        return Vec::new();
    };
    let temp_s = temp.to_string_lossy().into_owned();
    let jpeg = thumb_keyed(&state.preview_thumb_cache(), vpath, &temp_s, mtime, size, w).await;
    drop(export); // 导出件活到引擎面返回,Drop 清理临时件
    jpeg
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 图样(写面与断言面同源,T3 同款函数式)。
    fn pattern(i: u64) -> u8 {
        (i.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as u8
    }

    fn data(n: usize) -> Vec<u8> {
        (0..n as u64).map(pattern).collect()
    }

    /// 测试缓存(帽/目录注入)。
    fn cache(dir: &Path, cap: u64) -> ThumbCache {
        ThumbCache::with_params(dir, cap)
    }

    fn k(tag: &str) -> String {
        // 合法键域(纯字母数字)的固定测试键
        format!("key{tag}0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
            .chars()
            .take(64)
            .collect()
    }

    fn dir_names(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    // ---------- 缓存键 ----------

    /// 键稳定性/敏感性(结构断言,不钉具体 hex):同参同键;path/mtime/
    /// size/w 任一变更即新键;键域纯字母数字(文件系统安全)。
    #[test]
    fn cache_key_stability_and_sensitivity() {
        let k1 = cache_key("C:\\a\\p.jpg", 111, 222, 256);
        assert_eq!(k1, cache_key("C:\\a\\p.jpg", 111, 222, 256), "同参同键");
        assert_eq!(k1.len(), 64, "sha256 hex 64 位");
        assert!(
            k1.bytes().all(|b| b.is_ascii_alphanumeric()),
            "键域纯字母数字: {k1}"
        );
        assert_ne!(cache_key("C:\\a\\p2.jpg", 111, 222, 256), k1, "path 变");
        assert_ne!(cache_key("C:\\a\\p.jpg", 112, 222, 256), k1, "mtime 变");
        assert_ne!(cache_key("C:\\a\\p.jpg", 111, 223, 256), k1, "size 变");
        assert_ne!(cache_key("C:\\a\\p.jpg", 111, 222, 128), k1, "w 变");
        // 组件歧义闭合:`a`(mtime=0,size 拼接形态)与字面歧义构造异键
        assert_ne!(
            cache_key("a", 0, 1, 1),
            cache_key("a|0000000000000001", 0, 0, 0),
            "分隔歧义闭合"
        );
    }

    /// 常量锚(512 MiB;改值须过本断言)。
    #[test]
    fn cap_constant_anchor() {
        assert_eq!(THUMB_CACHE_CAP_BYTES, 512 * 1024 * 1024, "全局帽常量锚");
        assert_eq!(THUMB_MAX_WIDTH, 1024, "宽钳制常量锚");
    }

    // ---------- 缓存 LRU / 记账 ----------

    /// 存取往返 + LRU 读序刷新:store k1,k2;load k1(命中刷新)→ store
    /// k3 超帽 → 出清 k2(次旧,被 k1 的命中压后);无命中面(store 序)
    /// 对照 = 最旧 k1 先出。reload 幸存面与计费一致。
    #[test]
    fn store_load_roundtrip_lru_read_order() {
        let dir = tempfile::tempdir().unwrap();
        let c = cache(dir.path(), 100);
        assert!(c.store(&k("a"), &data(30)), "k1 入");
        assert!(c.store(&k("b"), &data(40)), "k2 入");
        assert_eq!(c.total_bytes(), 70);
        assert_eq!(c.load(&k("a")).unwrap(), data(30), "命中字节一致");
        assert_eq!(
            c.key_order(),
            vec![k("b"), k("a")],
            "命中把 k1 刷到最新(LRU 读序)"
        );
        // 90 > 帽:可淘汰面 = k2(次旧;k1 刚命中)
        assert!(c.store(&k("c"), &data(50)), "k3 入");
        assert_eq!(c.key_order(), vec![k("a"), k("c")], "k2 出清");
        assert!(c.load(&k("b")).is_none(), "出清键 miss");
        assert_eq!(c.total_bytes(), 80);

        // reload:落账持久,幸存序与计费一致
        let c2 = cache(dir.path(), 100);
        assert_eq!(c2.len(), 2);
        assert_eq!(c2.total_bytes(), 80);
        assert_eq!(c2.key_order(), vec![k("a"), k("c")]);
        assert_eq!(c2.load(&k("a")).unwrap(), data(30), "reload 字节一致");
    }

    /// 纯 store 序淘汰(无命中面):最旧先出(k1)。
    #[test]
    fn evict_pure_store_order_without_hits() {
        let dir = tempfile::tempdir().unwrap();
        let c = cache(dir.path(), 50);
        assert!(c.store(&k("a"), &data(30)));
        assert!(c.store(&k("b"), &data(30))); // 60 > 50 → k1 出
        assert_eq!(c.key_order(), vec![k("b")]);
        assert_eq!(c.total_bytes(), 30);
        assert!(
            dir_names(dir.path())
                .iter()
                .all(|n| n.as_str() != format!("{}{FILE_SUFFIX}", k("a"))),
            "盘面同步出清"
        );
    }

    /// 键卫生(T5 valid_handle 同款):分隔/点号/空串 store 拒、load None、
    /// 目录零写入;空字节 store 拒(空=失败面,不入缓存)。
    #[test]
    fn invalid_key_and_empty_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let c = cache(dir.path(), 100);
        for bad in ["a/b", "..", "", "a.b", "a\\b"] {
            assert!(!c.store(bad, &data(5)), "非法键 {bad:?} 必拒");
            assert!(c.load(bad).is_none(), "非法键 {bad:?} 查询 None");
        }
        assert!(!c.store(&k("a"), &[]), "空字节拒(失败面不入缓存)");
        assert!(
            dir_names(dir.path()).is_empty(),
            "零目录写入: {:?}",
            dir_names(dir.path())
        );
    }

    /// ghost 自愈:外力删文件 → load None(不剔账);同键再 store → 命中
    /// 恢复且计费随新件(不双计)。
    #[test]
    fn ghost_miss_heals_on_store() {
        let dir = tempfile::tempdir().unwrap();
        let c = cache(dir.path(), 100);
        assert!(c.store(&k("a"), &data(30)));
        std::fs::remove_file(dir.path().join(format!("{}{FILE_SUFFIX}", k("a")))).unwrap();
        assert!(c.load(&k("a")).is_none(), "外力删除 = miss");
        assert_eq!(c.total_bytes(), 30, "ghost 条目留账");
        assert!(c.store(&k("a"), &data(25)));
        assert_eq!(c.load(&k("a")).unwrap(), data(25), "store 自愈");
        assert_eq!(c.total_bytes(), 25, "替换计费不双计");
        assert_eq!(c.len(), 1);
    }

    /// 损坏兜底(T5 同款):坏账(真坏 JSON)备份 `.bad-<ts>` 留底后从空;
    /// 失账缓存件随对账清扫归零;备份件不被清扫误删;后续 store 照常。
    #[test]
    fn corrupt_ledger_backs_up_and_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(LEDGER_FILE), "{not json").unwrap();
        std::fs::write(
            dir.path().join(format!("{}{FILE_SUFFIX}", k("old"))),
            data(5),
        )
        .unwrap();
        let c = cache(dir.path(), 1000);
        assert_eq!(c.len(), 0, "损坏后从空开始");
        let names = dir_names(dir.path());
        let bads: Vec<_> = names
            .iter()
            .filter(|n| n.starts_with(&format!("{LEDGER_FILE}.bad-")))
            .collect();
        assert_eq!(bads.len(), 1, "恰一枚坏账备份: {names:?}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(bads[0])).unwrap(),
            "{not json",
            "备份原样留底"
        );
        assert!(
            !names.contains(&format!("{}{FILE_SUFFIX}", k("old"))),
            "失账缓存件随对账清扫归零: {names:?}"
        );
        assert!(c.store(&k("n"), &data(3)), "损坏兜底后 store 正常");
        assert!(c.load(&k("n")).is_some());
    }

    /// 对账清扫(启动初始化,T5 同款):账内缺席键剔账、实长校准、账外
    /// 孤儿/tmp 删除;目录条目与记账本件不碰。
    #[test]
    fn init_reconciles_and_sweeps_orphans() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(format!("{}{FILE_SUFFIX}", k("keep"))),
            data(5),
        )
        .unwrap();
        std::fs::write(dir.path().join("orphan.bin"), data(99)).unwrap();
        std::fs::write(dir.path().join("k.tmp-7-4242"), data(77)).unwrap();
        std::fs::create_dir_all(dir.path().join("子目录")).unwrap();
        let ledger = format!(
            r#"{{"entries":[{{"key":"{}","bytes":12345}},{{"key":"{}","bytes":9}}]}}"#,
            k("ghost"),
            k("keep")
        );
        std::fs::write(dir.path().join(LEDGER_FILE), ledger).unwrap();
        let c = cache(dir.path(), 1000);
        assert_eq!(c.len(), 1, "ghost 剔账");
        assert_eq!(c.total_bytes(), 5, "实长校准(账 9 → 盘 5)");
        assert!(c.load(&k("keep")).is_some());
        assert!(c.load(&k("ghost")).is_none());
        assert!(!dir.path().join("orphan.bin").exists(), "孤儿删除");
        assert!(!dir.path().join("k.tmp-7-4242").exists(), "tmp 残留删除");
        assert!(dir.path().join("子目录").is_dir(), "目录条目不碰");
        assert!(dir.path().join(LEDGER_FILE).exists(), "记账本件不碰");
    }

    // ---------- 引擎面(brief 三面;Windows 后端行为面) ----------

    /// JPEG 夹具(image crate 现编,免外部二进制;`color` 纯色可复算)。
    #[cfg(windows)]
    fn fixture_jpeg(dir: &Path, name: &str, w: u32, h: u32, color: [u8; 3]) -> (PathBuf, u64) {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |_x, _y| {
            image::Rgb(color)
        }));
        let path = dir.join(name);
        image::codecs::jpeg::JpegEncoder::new_with_quality(
            std::fs::File::create(&path).expect("夹具建文件"),
            90,
        )
        .encode_image(&img)
        .expect("JPEG 夹具现编");
        let size = std::fs::metadata(&path).unwrap().len();
        (path, size)
    }

    /// 夹具 → thumb 两次:第二次零重生成(计数不增 = 缓存命中,字节一致);
    /// mtime 变 → 重生成(计数 +1)。brief Step 1 第一面。
    #[cfg(windows)]
    #[tokio::test]
    async fn thumb_image_twice_cache_hit_mtime_change_regenerates() {
        let _serial = TEST_SERIAL.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let (path, size) = fixture_jpeg(dir.path(), "pic.jpg", 128, 96, [200, 30, 40]);
        let path_s = path.to_string_lossy().into_owned();
        let cache = ThumbCache::new(home.path());
        let gens = || GEN_CALLS.load(std::sync::atomic::Ordering::SeqCst);

        let g0 = gens();
        let t1 = thumb(&cache, &path_s, 111, size, 64).await;
        assert!(!t1.is_empty(), "图片出非空 JPEG");
        let img = image::load_from_memory(&t1).expect("JPEG 可解");
        assert!(
            img.width() <= 64 && img.height() <= 64,
            "等比入槽 ≤w: {}x{}",
            img.width(),
            img.height()
        );
        let g1 = gens();
        assert_eq!(g1, g0 + 1, "首取恰生成一次");

        let t2 = thumb(&cache, &path_s, 111, size, 64).await;
        assert_eq!(t2, t1, "二取缓存命中字节一致");
        assert_eq!(gens(), g1, "二取零重生成(计数不增)");

        // mtime 变 → 新键 → 重生成(同文件不同内容更可信;此处只钉计数)
        let t3 = thumb(&cache, &path_s, 222, size, 64).await;
        assert!(!t3.is_empty(), "mtime 变照常出新图");
        assert_eq!(gens(), g1 + 1, "mtime 变 → 重生成");
    }

    /// 视频文件出非空帧(brief Step 1 第二面;MJPEG AVI 夹具现编——
    /// image crate 出 JPEG 帧,手装 RIFF/AVI 容器,Windows 视频缩略图
    /// 管线(Shell→Media Foundation)对 MJPEG 有内建解码)。
    #[cfg(windows)]
    fn fixture_avi(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x * 4 % 256) as u8, (y * 5 % 256) as u8, 128])
        }));
        let mut frame = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut frame, 90)
            .encode_image(&img)
            .expect("JPEG 帧现编");
        let path = dir.join(name);
        std::fs::write(&path, build_mjpeg_avi(&frame, w, h)).expect("AVI 夹具写盘");
        path
    }

    /// 手装单帧 MJPEG AVI(RIFF 容器:hdrl(avih+strl(strh+strf))+
    /// movi(00dc=JPEG 帧)+ idx1;AVIF_HASINDEX + 关键帧旗标;idx1 偏移
    /// 按「相对 'movi' 数据区」约定)。仅测试夹具消费。
    #[cfg(windows)]
    fn build_mjpeg_avi(jpeg: &[u8], w: u32, h: u32) -> Vec<u8> {
        fn le32(v: u32) -> [u8; 4] {
            v.to_le_bytes()
        }
        fn le16(v: u16) -> [u8; 2] {
            v.to_le_bytes()
        }
        fn fourcc(s: &str) -> [u8; 4] {
            s.as_bytes().try_into().expect("fourcc 4 字节")
        }
        fn chunk(id: [u8; 4], payload: &[u8]) -> Vec<u8> {
            let mut out = id.to_vec();
            out.extend_from_slice(&le32(payload.len() as u32));
            out.extend_from_slice(payload);
            out
        }
        fn list(kind: &str, payload: &[&[u8]]) -> Vec<u8> {
            let mut body = fourcc(kind).to_vec();
            for part in payload {
                body.extend_from_slice(part);
            }
            let mut out = fourcc("LIST").to_vec();
            out.extend_from_slice(&le32(body.len() as u32));
            out.extend_from_slice(&body);
            out
        }
        // strh:vids/MJPG,1 帧(1e6 μs/帧)
        let mut strh = Vec::new();
        strh.extend_from_slice(&fourcc("vids"));
        strh.extend_from_slice(&fourcc("MJPG"));
        strh.extend_from_slice(&le32(0)); // dwFlags
        strh.extend_from_slice(&le16(0)); // wPriority
        strh.extend_from_slice(&le16(0)); // wLanguage
        strh.extend_from_slice(&le32(0)); // dwInitialFrames
        strh.extend_from_slice(&le32(1)); // dwScale
        strh.extend_from_slice(&le32(1)); // dwRate
        strh.extend_from_slice(&le32(0)); // dwStart
        strh.extend_from_slice(&le32(1)); // dwLength(1 帧)
        strh.extend_from_slice(&le32(jpeg.len() as u32 + 40)); // dwSuggestedBufferSize
        strh.extend_from_slice(&le32(10000)); // dwQuality
        strh.extend_from_slice(&le32(0)); // dwSampleSize
        strh.extend_from_slice(&[0u8; 16]); // rcFrame
                                            // strf:BITMAPINFOHEADER(40B;MJPEG 常规 biHeight 正值约定)
        let mut strf = Vec::new();
        strf.extend_from_slice(&le32(40)); // biSize
        strf.extend_from_slice(&le32(w)); // biWidth
        strf.extend_from_slice(&le32(h)); // biHeight
        strf.extend_from_slice(&le16(1)); // biPlanes
        strf.extend_from_slice(&le16(24)); // biBitCount
        strf.extend_from_slice(&fourcc("MJPG")); // biCompression
        strf.extend_from_slice(&le32(jpeg.len() as u32)); // biSizeImage
        strf.extend_from_slice(&le32(0)); // biXPelsPerMeter
        strf.extend_from_slice(&le32(0)); // biYPelsPerMeter
        strf.extend_from_slice(&le32(0)); // biClrUsed
        strf.extend_from_slice(&le32(0)); // biClrImportant
                                          // avih(主头)
        let mut avih = Vec::new();
        avih.extend_from_slice(&le32(1_000_000)); // dwMicroSecPerFrame
        avih.extend_from_slice(&le32(0)); // dwMaxBytesPerSec
        avih.extend_from_slice(&le32(0)); // dwPaddingGranularity
        avih.extend_from_slice(&le32(0x10)); // dwFlags: AVIF_HASINDEX
        avih.extend_from_slice(&le32(1)); // dwTotalFrames
        avih.extend_from_slice(&le32(0)); // dwInitialFrames
        avih.extend_from_slice(&le32(1)); // dwStreams
        avih.extend_from_slice(&le32(jpeg.len() as u32 + 40)); // dwSuggestedBufferSize
        avih.extend_from_slice(&le32(w)); // dwWidth
        avih.extend_from_slice(&le32(h)); // dwHeight
        avih.extend_from_slice(&[0u8; 16]); // dwReserved×4
                                            // movi:'movi' fourcc 之后是 00dc 帧;idx1 偏移相对 movi 数据区(帧
                                            // chunk 从 +4 起)
        let frame_chunk = chunk(fourcc("00dc"), jpeg);
        let mut movi = fourcc("movi").to_vec();
        movi.extend_from_slice(&frame_chunk);
        let mut idx1 = Vec::new();
        idx1.extend_from_slice(&fourcc("00dc"));
        idx1.extend_from_slice(&le32(0x10)); // AVIIF_KEYFRAME
        idx1.extend_from_slice(&le32(4)); // 相对 'movi' 数据区
        idx1.extend_from_slice(&le32(jpeg.len() as u32));
        let hdrl = list(
            "hdrl",
            &[
                &chunk(fourcc("avih"), &avih),
                &list(
                    "strl",
                    &[&chunk(fourcc("strh"), &strh), &chunk(fourcc("strf"), &strf)],
                ),
            ],
        );
        let mut riff = b"RIFF".to_vec();
        let body_len =
            4 + hdrl.len() + list("movi", &[&movi[4..]]).len() + chunk(fourcc("idx1"), &idx1).len();
        riff.extend_from_slice(&le32(body_len as u32));
        riff.extend_from_slice(&fourcc("AVI "));
        riff.extend_from_slice(&hdrl);
        riff.extend_from_slice(&list("movi", &[&movi[4..]]));
        riff.extend_from_slice(&chunk(fourcc("idx1"), &idx1));
        riff
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn thumb_video_produces_nonempty_frame() {
        let _serial = TEST_SERIAL.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let path = fixture_avi(dir.path(), "clip.avi", 96, 64);
        let size = std::fs::metadata(&path).unwrap().len();
        let cache = ThumbCache::new(home.path());
        let t = thumb(&cache, path.to_string_lossy().as_ref(), 5, size, 64).await;
        assert!(!t.is_empty(), "视频出非空帧(shell 缩略图管线)");
        let img = image::load_from_memory(&t).expect("JPEG 可解");
        assert!(
            img.width() <= 64 && img.height() <= 64,
            "帧等比入槽 ≤w: {}x{}",
            img.width(),
            img.height()
        );
    }

    /// 无后端文件类型 → 空字节(brief Step 1 第三面;SIIGBF_THUMBNAILONLY
    /// 对提不出缩略图的类型如实失败,绝不拿图标充数);空结果不入缓存。
    #[cfg(windows)]
    #[tokio::test]
    async fn thumb_unsupported_type_is_empty_bytes_not_cached() {
        let _serial = TEST_SERIAL.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.xyz");
        std::fs::write(&path, b"definitely not a thumbnailable file").unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        let path_s = path.to_string_lossy().into_owned();
        let cache = ThumbCache::new(home.path());
        let t = thumb(&cache, &path_s, 1, size, 64).await;
        assert!(t.is_empty(), "无后端类型 = 空字节(回退类型图标)");
        assert_eq!(cache.len(), 0, "空结果不入缓存(负缓存面)");
        assert_eq!(cache.total_bytes(), 0);
    }

    /// 桥 vpath 键/源分离面(评审 Important #1 回归,brief「第二次零重
    /// 生成=缓存命中」的桥臂形态):同一 vpath 两次(两次「导出」的临时
    /// 件路径不同、内容同)→ 第二次零重生成(GEN_CALLS 增量为零)+记账
    /// 恰一条——键不随临时件路径漂移,LRU 不被永不命中的唯一键灌满。
    #[cfg(windows)]
    #[tokio::test]
    async fn thumb_keyed_bridge_vpath_hits_across_distinct_export_temps() {
        let _serial = TEST_SERIAL.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        // 同一内容的两份「导出临时件」(两次导出,路径必不同)
        let (p1, s1) = fixture_jpeg(dir.path(), "exp-1.jpg", 96, 64, [10, 20, 30]);
        let (p2, _s2) = fixture_jpeg(dir.path(), "exp-2.jpg", 96, 64, [10, 20, 30]);
        assert_ne!(p1, p2, "两次导出的临时件路径不同(键漂移回归面前提)");
        let cache = ThumbCache::new(home.path());
        let gens = || GEN_CALLS.load(std::sync::atomic::Ordering::SeqCst);
        let vpath = "/桥根/photo.jpg";

        let g0 = gens();
        let t1 = thumb_keyed(&cache, vpath, p1.to_string_lossy().as_ref(), 7, s1, 64).await;
        assert!(!t1.is_empty(), "桥臂首取出非空 jpeg(从临时件生成)");
        assert_eq!(gens(), g0 + 1, "首取恰生成一次");
        assert_eq!(cache.len(), 1, "记账恰一条");

        // 同一 vpath 第二次(新导出临时件 p2):命中零重生成
        let t2 = thumb_keyed(&cache, vpath, p2.to_string_lossy().as_ref(), 7, s1, 64).await;
        assert_eq!(t2, t1, "命中字节一致");
        assert_eq!(gens(), g0 + 1, "二取零重生成(键不随临时件漂移)");
        assert_eq!(cache.len(), 1, "仍恰一条记账(无漂移唯一键)");

        // 对照:vpath 变(mtime 同/文件同)= 新键,重生成(键敏感性照常)
        let t3 = thumb_keyed(
            &cache,
            "/桥根/另图.jpg",
            p2.to_string_lossy().as_ref(),
            7,
            s1,
            64,
        )
        .await;
        assert!(!t3.is_empty());
        assert_eq!(gens(), g0 + 2, "异 vpath = 新键重生成");
        assert_eq!(cache.len(), 2);
    }

    /// 非 Windows:无后端恒空字节(V4-3 波补前的回退面)。
    #[cfg(not(windows))]
    #[tokio::test]
    async fn thumb_no_backend_platform_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let path = dir.path().join("pic.jpg");
        std::fs::write(&path, b"\xff\xd8 not really a jpeg").unwrap();
        let cache = ThumbCache::new(home.path());
        let t = thumb(&cache, path.to_string_lossy().as_ref(), 1, 100, 64).await;
        assert!(t.is_empty(), "非 Windows 无后端 = 空字节");
        assert_eq!(cache.len(), 0, "空结果不入缓存");
    }

    /// 回传路径前缀判定矩阵(unix 形态;photo serve_dir_prefix_matrix 同款
    /// ——生产面只在安卓引擎,测试与 e2e 同 unix):前缀内(含深层/目录
    /// 本身)识别;兄弟目录字节前缀/越出根/相对/空不识别(fail-closed)。
    #[cfg(unix)]
    #[test]
    fn thumb_serve_dir_prefix_matrix() {
        let home = tempfile::tempdir().unwrap();
        let under = |p: &str| path_under_thumb_serve_dir(home.path(), Path::new(p));
        let root = thumb_serve_dir(home.path());
        let root_s = root.to_string_lossy().into_owned();
        assert!(under(&format!("{root_s}/1-x.jpg")), "前缀内识别");
        assert!(under(&format!("{root_s}/子/1-x.jpg")), "深层识别");
        assert!(under(&root_s), "目录本身在前缀内");
        assert!(
            !under(&format!("{}/thumb-serve-x/1.jpg", home.path().display())),
            "兄弟目录(字节前缀)不识别"
        );
        assert!(!under("/etc/passwd"), "越出根不识别");
        assert!(!under("相对/路径"), "相对不识别");
        assert!(!under(""), "空不识别");
    }

    /// daemon 集成点(最小接线):DaemonState::load 即完成 thumb-tmp 目录
    /// 初始化(`{home}/thumb-tmp` 在场)与缓存随 state 可取。
    #[test]
    fn daemon_state_initializes_thumb_cache() {
        let (state, home) = test_state_with_home();
        assert!(home.path().join("thumb-tmp").is_dir(), "启动即建缓存目录");
        let c = state.preview_thumb_cache();
        assert!(c.load("无").is_none(), "空缓存查询 None");
        assert!(c.store(&k("i"), &data(3)), "state 侧缓存可存");
        assert!(
            home.path()
                .join("thumb-tmp")
                .join(format!("{}{FILE_SUFFIX}", k("i")))
                .exists(),
            "缓存落 home 同源目录"
        );
    }

    /// 测试态(独立临时 home;preview.rs tests 同款装配面单点;保活
    /// TempDir 供目录断言)。
    fn test_state_with_home() -> (std::sync::Arc<crate::state::DaemonState>, tempfile::TempDir) {
        let home = tempfile::tempdir().unwrap();
        let (events, _) = tokio::sync::broadcast::channel(16);
        #[cfg(windows)]
        let update = std::sync::Arc::new(crate::update::UpdateService::new(
            crate::update::UpdateConfig::default(),
            events.clone(),
            tokio::sync::watch::channel(false).0,
        ));
        #[cfg(not(windows))]
        let update = ();
        let state = std::sync::Arc::new(
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
