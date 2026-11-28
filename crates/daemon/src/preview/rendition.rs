//! 转码会话引擎面(V4-1 T12):协议 + 源端会话管理 + 查看端装配骨架。
//!
//! 三段职责(一文件一职责面):
//! - **源端会话管理**([`RenditionSessionMgr`]):`open(source,config) ->
//!   session_id` / `segment(session,index)` / `cancel(session)`;空闲
//!   [`RENDITION_IDLE_TTL`] 回收(Review#2:注入时钟可测——活动时钟经
//!   构造注入,回收判定全走注入钟,测试拨钟即回收,零真实等待);清扫由
//!   open/segment/cancel 懒触发 + daemon 侧 ticker 兜底(无人再发 op 的
//!   长闲窗);回收即 `Transcoder::cancel`(App 侧转码资源随会话释放)。
//! - **转码后端 trait**([`Transcoder`]):v1 实现 = 测试伪转码器
//!   ([`PseudoTranscoder`],cfg(test))+ Android 桥后端
//!   ([`bridge_backend`],V4-1 T13:Kotlin MediaCodec 管线,0x0C 转码通道
//!   驱动,段文件落 `{home}/rendition-serve/{session}/` 交引擎读;生产装
//!   配 = Daemon::start 的 `target_os = "android"` 门,e2e 经 doc(hidden)
//!   钩子同语装配);iOS 真后端 T14 挂 [`platform_backend`] 缝;桌面
//!   (Windows)v1 无后端 → `open` 回稳定词 `unsupported` → 查看端回退
//!   原流+海报(T15 面,留票)。
//! - **查看端装配**([`ViewerRendition`] + [`RenditionFetch`]):按
//!   session 拉段入 T5 落盘缓存、合成动态 HLS playlist
//!   (`#EXT-X-VERSION:7`/fMP4/`#EXT-X-MAP`),经 loopback /f/{h}/video/
//!   出(T15 实装:playlist 路由=前缀推进+列表;seg 路由=缓存命中/跳段
//!   有界等待 10s→503;Review#1)。跳段语义(brief 钉死):段请求缺 =
//!   **向源端直接请求该段**(乱序直取,不经顺序追赶;「取消当前拉取序,
//!   改拉目标段起」=拉取序控制,见 [`ViewerRendition::pull_segments`] 与
//!   [`ViewerRendition::ensure_segment`] 的分工注)。
//!
//! 段错误稳定词(T12-M2,T15 钉死**两可区分词,不得共词**):
//! - **段未就绪(等待面)** = `segment_pending`(管线生产中,同段稍后重试
//!   可达;查看端入有界等待窗)
//! - **段越界(止步面)** = `segment_range`(本会话不会再产出该段;查看端
//!   即止步,勿烧等待窗)
//! 拉段错误分类(T12-M5):source_offline/会话回收等失败面**先于**重试/
//! 跳段逻辑分类,勿吞为段尽。
//!
//! 协议面(brief 钉死):rendition 三 op(rendition_open/segment/cancel)
//! 走 ctrl 分发缝([`super::handle_ctrl_request_reply`],置换 T1 留的
//! INTERNAL 桩;旧端未知变体 → 缺 body CtrlError,T1/T2 已备互通语义);
//! open 的路径路由单快照对齐 read_range 口径(桥 vpath → 桥导轨 seek
//! 先行/导出回退;照片 vpath → 照片导轨;其余 std::fs),守卫照片/桥
//! 直通臂对齐 T9/T10 口径(ctrl.rs)。
//!
//! 段序约定(T12 钉死,T13/T14/T15 共同遵守):`segment(index)` 的
//! **index 0 = init 段**(fMP4 头,playlist 的 `#EXT-X-MAP` 面,URI
//! `seg/0.m4s`),`index ≥ 1` = 内容段(URI `seg/{i}.m4s`)——一段线上
//! 请求面(rendition_segment)同时承载 init 与内容段。
//!
//! 远跳重开(V4-1 T15b,spec §5.1「以 start_at 就近关键帧重启转码」的
//! 贯通面):远目标段(如 90% 处)若靠 v1 乱序直取,等待 ≈ 整段转码时长,
//! 10s 界必 503。等待路径窗过半仍未达 = **cancel 当前会话 + 携
//! `start_at_ms = 目标段×段长` 重开**(`RenditionOpenRequest.start_at_ms`,
//! proto 只增字段 5;T13/T14 App 侧桥已端到端实现 start_at——
//! SEEK_TO_PREVIOUS_SYNC / sampleIsFullSync 就近关键帧重产)→ 余窗续等
//! (总界 10s 不变);playlist 按新会话重建(自重开锚列段,段索引空间
//! 沿用不重置);并发等待者经串行门 + 会话代次判重收敛为一枚新会话。
//!
//! 后端 `segment` 语义 = **读已产段**:实现按段粒度出流,慢产段(App
//! 转码追不上)诚实 Err(段未就绪)——T15 的「段请求缺 → 触发源端跳段,
//! 有界等待 10s → 503」在本 Err 面上收口。段读是阻塞调用,应答臂以
//! spawn_blocking 收口,流以有界通道泵(T2 帧序,纪律同 read_range 泵)。

use std::io;
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mtdss_core::transport::ctrl::{CtrlChunk, CtrlReply, CtrlStreamOut};
use mtdss_proto::ctrl::{
    ctrl_response, CtrlResponse, RenditionCancel, RenditionOpenRequest, RenditionOpenResponse,
    RenditionSegmentRequest, RenditionTarget,
};

use super::limiter;
use super::source::{self, FileSource};
use crate::ctrl::err_response;
use crate::state::DaemonState;
// 查看端 Local 臂直驱分发缝的请求构造 import(生产臂 T15:本机 device
// 的视频装配走进程内缝;测试面同语)。
use mtdss_proto::ctrl::{ctrl_request, CtrlRequest};

// ---------- 协议常量(spec §4 v1) ----------

/// 转码目标形态常量(v1 唯一形态):段长 4000ms / 1080p / 4–6 Mbps。
pub(crate) struct RenditionConfig {
    /// 段长(毫秒;RenditionOpenResponse.segment_ms 回给查看端)。
    pub(crate) segment_ms: u32,
    /// 目标高度(1080p;后端管线入参)。
    #[allow(dead_code)] // 后端入参(T13/T14 挂入);v1 常量锚
    pub(crate) height: u32,
    /// 目标码率窗(bps;后端管线入参)。
    #[allow(dead_code)] // 同上
    pub(crate) bitrate_bps: (u32, u32),
}

impl RenditionConfig {
    pub(crate) const SEGMENT_MS: u32 = 4000;
    pub(crate) const HEIGHT: u32 = 1080;
    pub(crate) const BITRATE_MIN_BPS: u32 = 4_000_000;
    pub(crate) const BITRATE_MAX_BPS: u32 = 6_000_000;
    /// v1 唯一形态(常量锚;改值须过本文件常量断言测试)。
    pub(crate) const DEFAULT: Self = Self {
        segment_ms: Self::SEGMENT_MS,
        height: Self::HEIGHT,
        bitrate_bps: (Self::BITRATE_MIN_BPS, Self::BITRATE_MAX_BPS),
    };
}

/// 段 MIME(fMP4;proto RenditionOpenResponse.mime 注)。
pub(crate) const SEGMENT_MIME: &str = "video/iso.segment";

// ---------- 段错误稳定词(T12-M2:两可区分词,不得共词) ----------

/// 段未就绪(等待面):管线生产中,同段稍后重试可达——`segment_pending`
/// 稳定词的 Err 原料(InvalidInput 类;查看端入有界等待窗)。消费面 =
/// AndroidTranscoder(unix 门)与测试伪转码器。
#[cfg_attr(all(not(unix), not(test)), allow(dead_code))]
pub(crate) fn err_segment_pending(index: u32) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("段未就绪(等待,生产中): {index}"),
    )
}

/// 段越界标记(T15-M2 止步面):私有错误类型,`io::Error::downcast_ref`
/// 类型级判据——本工具链 `ErrorKind` 无 OutOfRange 变体(io_error_more
/// 未稳),勿以词面解析/kind 误植代替类型判据。
#[derive(Debug)]
struct SegmentRangeError(u32);

impl std::fmt::Display for SegmentRangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "段越界(止步): {}", self.0)
    }
}

impl std::error::Error for SegmentRangeError {}

/// 段越界(止步面):本会话不会再产出该段(段尽)——`segment_range`
/// 稳定词的 Err 原料(标记类型;查看端即止步,勿烧等待窗)。v1 消费面 =
/// 测试伪转码器(生产后端 = 桥管线无总段数概念,恒不报越界;总段数
/// 已知后端挂入时即真消费)。
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn err_segment_range(index: u32) -> io::Error {
    io::Error::other(SegmentRangeError(index))
}

/// 段取错误是否止步面(越界标记在身;借 `get_ref` 走 `dyn Error` 的
/// downcast 面——io::Error 本体的 downcast_ref 本工具链缺席)。
fn is_segment_range(e: &io::Error) -> bool {
    e.get_ref()
        .and_then(|er| er.downcast_ref::<SegmentRangeError>())
        .is_some()
}

/// 空闲回收界(Review#2;常量锚,测试注入时钟在其上断言)。
pub(crate) const RENDITION_IDLE_TTL: Duration = Duration::from_secs(60);

// ---------- 转码后端 trait + 装配缝 ----------

/// 转码后端(源端会话的背后引擎):把已就位的数据源切成 fMP4 段。实现须
/// Send+Sync(段读在 spawn_blocking 阻塞池上逐块泵);`cancel` 幂等
/// (显式 cancel/空闲回收/daemon 收口三处都走到,重复无害)。
pub(crate) trait Transcoder: Send + Sync {
    /// 源总时长毫秒(r29:open 回执携带;缺省 0 = 未知——查看端落
    /// EVENT 生长形)。
    fn duration_ms(&self) -> u64 {
        0
    }

    /// 取段流(index 0 = init;越界/段未就绪 = Err)。返回的读面在阻塞
    /// 池上被逐块泵读,须 Send。段尽由调用方拉至 Err 止步探测(动态
    /// playlist 的「未产段不列」面),后端无须另报总段数。
    fn segment(&self, index: u32) -> io::Result<Box<dyn io::Read + Send>>;
    /// 取消并释放后端资源(MediaCodec/VideoToolbox 管线)。
    fn cancel(&self);
}

/// 后端装配工厂:源就位后造后端(管线启动含在造面);None = 本端无转码
/// 后端(open 回稳定词 unsupported)。工厂在会话表锁**外**调用(App 桥
/// 启动可秒级,不得占表阻塞 segment/cancel)。入参 = (源面, 源 vpath,
/// start_at_ms)——App 桥后端(V4-1 T13)以 vpath 让 App 按 seek/thumb 桥
/// 同源分流自行开源转码(V4-1 扩:T12 原形仅源面;桥后端需要 App 侧可
/// 解析的路由键);start_at_ms = 起点毫秒(V4-1 T15b:远跳重开原料,0 =
/// 从头;T13/T14 桥事件载荷已有该字段,本缝透传即贯通),桌面实现三参
/// 皆忽略。
pub(crate) type BackendFactory =
    Arc<dyn Fn(Arc<dyn FileSource>, &str, u64) -> Option<Arc<dyn Transcoder>> + Send + Sync>;

/// 平台后端装配(生产面):v1 桌面全线无后端 → 恒 None(open 回
/// unsupported,查看端回退原流+海报);Android 真后端(V4-1 T13
/// MediaCodec 桥)与 iOS 真后端(V4-1 T14 AVAssetReader+VideoToolbox 桥,
/// 同一 0x0C 线形)由 Daemon::start 经
/// [`RenditionSessionMgr::install_factory`] 挂入([`bridge_backend`])。
pub(crate) fn platform_backend() -> BackendFactory {
    Arc::new(|_source, _path, _start_at_ms| None)
}

/// App 转码段临时目录锚(`{home}/rendition-serve`;与 Kotlin 侧
/// MtdssEngine.renditionServeDir 同锚,两侧不得漂移——thumb-serve 同构
/// 三面:App 落盘 / 引擎读 / App 挂载清扫;目录本体归 App 建与清,引擎
/// 只读不删)。
#[cfg_attr(not(unix), allow(dead_code))] // 消费面=桥后端/装配钩(unix 门)
pub(crate) fn rendition_serve_dir(home: &std::path::Path) -> PathBuf {
    home.join("rendition-serve")
}

/// 回传段路径是否落在 App 转码临时目录的**本会话目录**内
/// (`{serve}/{session}/`;thumb 同款防逃逸 + 会话域收紧:App 侧 bug 不得
/// 诱使引擎读前缀外/他会话文件。词法面:规范化后整组件前缀,非 UTF-8/
/// 非法形态一律不在 fail-closed)。
#[cfg(unix)]
fn path_under_session_dir(serve_dir: &std::path::Path, session: u64, p: &Path) -> bool {
    match (
        crate::roots::normalize_lexical(&serve_dir.join(session.to_string())),
        crate::roots::normalize_lexical(p),
    ) {
        (Ok(dir), Ok(path)) => path.starts_with(&dir),
        _ => false,
    }
}

/// Android 桥后端工厂(V4-1 T13):App 桥([`crate::jobs::RenditionBridge`])
/// 驱动 Kotlin MediaCodec 管线——工厂调用 = 开桥会话(start_at_ms 透传
/// 桥 open 的 fs.rendition_open 事件载荷,T15b 远跳重开即此贯通:查看端
/// 重开 = 新工厂调用携目标段起点,App 侧就近关键帧重开管线),成功 = 会
/// 话键(session = 桥 request_id)铸 [`AndroidTranscoder`];**桥不可用/
/// 超时/拒答一律落 None**(工厂缝无错误通道,T12 钉死)→ open 回
/// unsupported → 查看端回退原流+海报(降级面诚实:失败词面走 tracing
/// 日志消费)。工厂闭包在会话表锁外、spawn_blocking 阻塞池上调用(评审
/// Important #1 同纪律),内部 `Handle::block_on` 驱动桥的异步 open
/// (oneshot 有界等待,不占异步 worker)。unix 门:fdpass 通道面(Windows
/// 编译单元整体缺席,占位桥恒 Err,本工厂不装配)。
#[cfg(unix)]
pub(crate) fn bridge_backend(
    bridge: std::sync::Arc<crate::jobs::RenditionBridge>,
    serve_dir: PathBuf,
) -> BackendFactory {
    Arc::new(move |source, path, start_at_ms| {
        let opened = tokio::runtime::Handle::try_current()
            .ok()
            .and_then(|h| h.block_on(bridge.open(path, start_at_ms)).ok());
        match opened {
            Some((session, duration_ms)) => {
                // 源面随手柄落放(桥后端不经 FileSource 读源——App 按
                // vpath 自行开源转码;source 入参仅供签名一致性/桌面实现)
                drop(source);
                Some(Arc::new(AndroidTranscoder {
                    bridge: Arc::clone(&bridge),
                    session,
                    duration_ms,
                    serve_dir: serve_dir.clone(),
                }) as Arc<dyn Transcoder>)
            }
            None => {
                tracing::warn!(path, "转码桥会话开启失败,转码后端缺席(unsupported)");
                None
            }
        }
    })
}

/// Android 桥后端(V4-1 T13):段读 = 桥记账路径开文件(「读已产段」;
/// 段未回告 = InvalidInput「段未就绪(等待)」T15-M2 稳定词——T15 有界
/// 等待/503 在本 Err 面收口;管线失败标记在场时新段即 Err 勿烧等待);
/// 段文件生命周期归 App(cancel 即清,引擎只读不删——重复拉段/重试照读)。
/// cancel = 桥发后即忘广播(App 停管线+清段文件)。
#[cfg(unix)]
struct AndroidTranscoder {
    bridge: std::sync::Arc<crate::jobs::RenditionBridge>,
    session: u64,
    /// 源总时长毫秒(r29:open 回执携带;0 = 未知)。
    duration_ms: u64,
    serve_dir: PathBuf,
}

#[cfg(unix)]
impl Transcoder for AndroidTranscoder {
    fn duration_ms(&self) -> u64 {
        self.duration_ms
    }

    fn segment(&self, index: u32) -> io::Result<Box<dyn io::Read + Send>> {
        let path = match self.bridge.segment_path(self.session, index) {
            Some(p) => p,
            None if self.bridge.is_complete(self.session) => {
                // field 修:管线自然完毕(0x0C eof)——越界段答段越界(止步
                // 面),查看端 playlist 据此封 ENDLIST;不再烧等待窗
                return Err(err_segment_range(index));
            }
            None if self.bridge.failure(self.session).is_some() => {
                // 管线已失败:未产段必不再来,即 Err 勿烧 T15 等待窗
                let why = self.bridge.failure(self.session).unwrap_or_default();
                return Err(io::Error::other(format!("转码管线已失败(段 {index} 未就绪): {why}")));
            }
            None => return Err(err_segment_pending(index)),
        };
        // 回传路径前缀校验 fail-closed(拒读前缀外/他会话路径;thumb 同款)
        if !path_under_session_dir(&self.serve_dir, self.session, &path) {
            return Err(io::Error::other(format!(
                "App 段回告路径越出会话临时目录({}): {}",
                self.serve_dir.display(),
                path.display()
            )));
        }
        Ok(Box::new(std::fs::File::open(&path)?))
    }

    fn cancel(&self) {
        self.bridge.cancel(self.session);
    }
}

/// open 失败面(调用方=应答臂映射稳定词;测试面断言)。
#[derive(Debug)]
pub(crate) enum OpenError {
    /// 本端无转码后端 → 稳定词 `unsupported`。
    Unsupported,
    /// 会话开启 IO 面(源/管线)→ `source_offline`。
    Io(io::Error),
}

// ---------- 源端会话管理 ----------

/// 活动时钟(构造注入;生产 = `Instant::now`,测试注入假钟——Review#2
/// 可测面:回收判定全走注入钟)。
type NowFn = Arc<dyn Fn() -> std::time::Instant + Send + Sync>;

/// 会话记录(锁内;`last_used` = 空闲判定锚,拉段即刷新)。segment_ms/
/// mime 的读面 = `meta`(测试/查询缝);open 应答臂直取常量组装。
struct SessionRec {
    backend: Arc<dyn Transcoder>,
    #[cfg_attr(not(test), allow(dead_code))] // 读面 = meta(测试缝)
    segment_ms: u32,
    #[cfg_attr(not(test), allow(dead_code))] // 同上
    mime: &'static str,
    last_used: std::time::Instant,
}

#[derive(Default)]
struct MgrState {
    next: u64,
    sessions: std::collections::HashMap<u64, SessionRec>,
}

/// 源端转码会话管理器(随 DaemonState 常驻,进程内单份):会话表 +
/// 空闲回收 + 后端装配缝。std 锁不跨 await(open/segment/cancel/sweep
/// 全同步;后端调用面在应答臂的 spawn_blocking 上)。
pub(crate) struct RenditionSessionMgr {
    inner: Mutex<MgrState>,
    /// 后端装配缝(独立锁:工厂克隆出锁外再调用,见 [`BackendFactory`] 注;
    /// 测试经 `install_factory` 置伪转码器)。
    factory: Mutex<BackendFactory>,
    /// 空闲回收界(构造注入;生产 = [`RENDITION_IDLE_TTL`])。
    idle_ttl: Duration,
    /// 活动时钟(见 [`NowFn`])。
    now: NowFn,
}

impl RenditionSessionMgr {
    /// 生产构造(常量锚:60s 界 + 真实时钟 + 平台后端缝)。
    pub(crate) fn new() -> Self {
        Self::with_params(RENDITION_IDLE_TTL, Arc::new(std::time::Instant::now))
    }

    fn with_params(idle_ttl: Duration, now: NowFn) -> Self {
        Self {
            inner: Mutex::new(MgrState::default()),
            factory: Mutex::new(platform_backend()),
            idle_ttl,
            now,
        }
    }

    /// 测试构造:界/钟可注(回收用例零真实等待)。
    #[cfg(test)]
    fn with_clock(idle_ttl: Duration, now: NowFn) -> Self {
        Self::with_params(idle_ttl, now)
    }

    /// 后端装配缝置换(V4-1 T13 起双消费面:测试面注入伪转码器;生产面
    /// Android 装配 App 桥真后端——Daemon::start 的 `target_os = "android"`
    /// 门内,与 e2e 的 doc(hidden) 钩子同语。置换整体生效,重复装无害。
    /// Windows 正式面无调用方,allow 沿 house 口径)。
    #[cfg_attr(all(not(unix), not(test)), allow(dead_code))]
    pub(crate) fn install_factory(&self, f: BackendFactory) {
        *self.factory.lock().unwrap() = f;
    }

    /// 开转码会话(源就位 → 后端装配(锁外)→ 会话登记)。懒清扫顺带。
    /// Err(Unsupported) = 稳定词 unsupported;Err(Io) = 源/管线面。
    /// `path` = 源 vpath(原词透传后端工厂;App 桥后端的开会话路由键)。
    /// `start_at_ms` = 起点毫秒(T15b:原词透传工厂→桥 open;每次 open =
    /// 新会话——同会话重复 open 不同 start_at 即 App 侧重开管线)。
    pub(crate) fn open(
        &self,
        source: Arc<dyn FileSource>,
        config: &RenditionConfig,
        path: &str,
        start_at_ms: u64,
    ) -> Result<(u64, u64), OpenError> {
        self.sweep();
        let factory = Arc::clone(&self.factory.lock().unwrap());
        let backend = (factory)(source, path, start_at_ms).ok_or(OpenError::Unsupported)?;
        let duration_ms = backend.duration_ms();
        let mut st = self.inner.lock().unwrap();
        let id = st.next;
        st.next = st
            .next
            .checked_add(1)
            .ok_or_else(|| OpenError::Io(io::Error::other("会话 id 分配耗尽")))?;
        st.sessions.insert(
            id,
            SessionRec {
                backend,
                segment_ms: config.segment_ms,
                mime: SEGMENT_MIME,
                last_used: (self.now)(),
            },
        );
        Ok((id, duration_ms))
    }

    /// 会话元信息((段长, mime);open 应答臂自 open 返回值组装,本读面
    /// 供测试/未来查询缝)。
    #[cfg(test)]
    pub(crate) fn meta(&self, session: u64) -> Option<(u32, &'static str)> {
        let st = self.inner.lock().unwrap();
        st.sessions.get(&session).map(|r| (r.segment_ms, r.mime))
    }

    /// 取段(index 0 = init;越界 = InvalidInput、会话不存在/已回收 =
    /// NotFound,应答臂映射稳定词)。拉段即刷新活动钟(空闲判定只看真
    /// 无人消费的窗)。
    pub(crate) fn segment(&self, session: u64, index: u32) -> io::Result<Box<dyn io::Read + Send>> {
        self.sweep();
        let mut st = self.inner.lock().unwrap();
        let rec = st
            .sessions
            .get_mut(&session)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "转码会话不存在或已回收"))?;
        rec.last_used = (self.now)();
        rec.backend.segment(index)
    }

    /// 取消会话(显式):摘表 + 后端 cancel(锁外释放)。返回是否存在
    /// (幂等面:线上 cancel 恒空 ok,未知会话照成)。
    pub(crate) fn cancel(&self, session: u64) -> bool {
        let backend = {
            let mut st = self.inner.lock().unwrap();
            st.sessions.remove(&session).map(|r| r.backend)
        };
        match backend {
            Some(b) => {
                b.cancel();
                true
            }
            None => false,
        }
    }

    /// 空闲清扫:超 [`Self::idle_ttl`] 未活动的会话一律摘表 + 后端
    /// cancel(锁外释放;App 侧转码资源随会话回收)。返回回收数。
    pub(crate) fn sweep(&self) -> usize {
        let now = (self.now)();
        let expired: Vec<Arc<dyn Transcoder>> = {
            let mut st = self.inner.lock().unwrap();
            let ids: Vec<u64> = st
                .sessions
                .iter()
                .filter(|(_, r)| now.duration_since(r.last_used) >= self.idle_ttl)
                .map(|(k, _)| *k)
                .collect();
            ids.into_iter()
                .filter_map(|id| st.sessions.remove(&id).map(|r| r.backend))
                .collect()
        };
        let n = expired.len();
        for b in expired {
            b.cancel();
        }
        n
    }

    /// 常驻 ticker(daemon 启动挂入,Daemon 收口 abort):定期清扫,兜
    /// 「无人再发起任何 rendition op」的长闲窗(懒回收只在有请求时触发)。
    pub(crate) fn spawn_sweeper(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let mgr = Arc::clone(self);
        tokio::spawn(async move {
            let pace = mgr.idle_ttl / 4;
            loop {
                tokio::time::sleep(pace).await;
                mgr.sweep();
            }
        })
    }

    /// daemon 收口:全部会话即刻回收(App 侧转码资源随 daemon 退场释放)。
    pub(crate) fn cancel_all(&self) {
        let backends: Vec<Arc<dyn Transcoder>> = {
            let mut st = self.inner.lock().unwrap();
            st.sessions.drain().map(|(_, r)| r.backend).collect()
        };
        for b in backends {
            b.cancel();
        }
    }

    /// 在册会话数(测试面:回收/detach 断言)。
    #[cfg(test)]
    pub(crate) fn live_sessions(&self) -> usize {
        self.inner.lock().unwrap().sessions.len()
    }
}

// ---------- 源端 ctrl 应答臂(分发缝置换 T1 桩) ----------

/// `rendition_open` 应答(守卫已过):**target 校验先行**(T12-M4:未知
/// target 拒答,勿静默按 HLS——新端新形态投旧端,稳定词
/// `unsupported_target` 供查看端定向降级,与「本端无后端」的
/// `unsupported` 可区分)→ 路径路由单快照对齐 read_range 口径(桥 vpath
/// → 桥导轨 seek 先行/导出回退;照片 vpath → 照片导轨;其余 std::fs)→
/// 声明大小核对(不符 = 文件已变更,NOT_FOUND,请求方重新浏览取新键)→
/// 会话开启(阻塞池收口)。无后端 = 稳定词 `unsupported` 单帧(查看端回
/// 退原流+海报);源面失败 = source_offline(预览面同码)。
pub(crate) async fn rendition_open_reply(
    state: Arc<DaemonState>,
    r: RenditionOpenRequest,
) -> CtrlReply {
    // target 校验(v1 唯一形态 HLS=0;proto3 未知枚举值原样保留在 i32)
    if RenditionTarget::try_from(r.target) != Ok(RenditionTarget::Hls) {
        return CtrlReply::Frame(err_response(
            "unsupported_target",
            format!("未知转码目标形态(target={};本端仅支持 HLS)", r.target),
        ));
    }
    // 单快照路由(与 read_range_reply 同款纪律:分发缝守卫与此处 route
    // 各取一次 roots 快照,不一致时以守卫为准——route 未命中落 fs 面由
    // NOT_FOUND 诚实报错)。
    let opened = match state.guard().bridge_route(&r.path) {
        Some(hit) => super::route_bridge(&state, &r.path, &hit).await,
        None if crate::photo::photo_vpath(&r.path).is_some() => {
            super::route_photo(&state, &r.path).await
        }
        None => match source::open_source("", &r.path, r.mtime, r.size as u64) {
            Ok(src) => Ok(src as Arc<dyn FileSource>),
            Err(e) => Err(err_response("source_offline", format!("源打开失败: {e}"))),
        },
    };
    let source = match opened {
        Ok(s) => s,
        Err(frame) => return CtrlReply::Frame(frame),
    };
    if r.size > 0 && source.size() != r.size as u64 {
        return CtrlReply::Frame(err_response(
            "NOT_FOUND",
            format!(
                "文件已变更(size {} ≠ 源 {}),请重新浏览",
                r.size,
                source.size()
            ),
        ));
    }
    // 会话开启(含懒清扫 + 后端工厂——App 桥启动可秒级)在阻塞池收口,
    // 不占异步 worker(segment/cancel 两臂同款纪律;评审 Important #1)。
    // start_at_ms 原词透传(T15b:查看端远跳重开 = 新 open 携目标段起点,
    // 工厂→桥 open→fs.rendition_open 事件载荷,App 侧就近关键帧重产)。
    let sessions = state.rendition_sessions();
    let path = r.path.clone();
    let start_at_ms = r.start_at_ms;
    let opened =
        tokio::task::spawn_blocking(move || {
            sessions.open(source, &RenditionConfig::DEFAULT, &path, start_at_ms)
        })
        .await;
    match opened {
        Ok(Ok((session_id, duration_ms))) => CtrlReply::Frame(CtrlResponse {
            version: mtdss_proto::CTRL_VERSION,
            ok: true,
            body: Some(ctrl_response::Body::RenditionOpen(RenditionOpenResponse {
                session_id,
                segment_ms: RenditionConfig::SEGMENT_MS,
                mime: SEGMENT_MIME.into(),
                duration_ms,
            })),
        }),
        Ok(Err(OpenError::Unsupported)) => CtrlReply::Frame(err_response(
            "unsupported",
            "本端无视频转码后端(桌面 v1 缺席),查看端回退原流+海报".into(),
        )),
        Ok(Err(OpenError::Io(e))) => CtrlReply::Frame(err_response(
            "source_offline",
            format!("转码会话开启失败: {e}"),
        )),
        Err(join) => CtrlReply::Frame(err_response(
            "INTERNAL",
            format!("会话开启任务收尾失败: {join}"),
        )),
    }
}

/// `rendition_segment` 应答(守卫直通臂已过;无本机路径面):会话取段 →
/// 流式泵(begin/chunk*/end,T2 帧序)。后端段读是阻塞调用 →
/// spawn_blocking 收口;预览档令牌随泵任务终了归还(与 read_range 同桶)。
pub(crate) async fn rendition_segment_reply(
    state: Arc<DaemonState>,
    r: RenditionSegmentRequest,
) -> CtrlReply {
    let mgr = state.rendition_sessions();
    let got = tokio::task::spawn_blocking(move || mgr.segment(r.session_id, r.index)).await;
    let reader = match got {
        Ok(Ok(reader)) => reader,
        Ok(Err(e)) => return CtrlReply::Frame(segment_err_response(&e)),
        Err(join) => {
            return CtrlReply::Frame(err_response(
                "INTERNAL",
                format!("段读任务收尾失败: {join}"),
            ));
        }
    };
    let permit = state
        .preview_limiter()
        .acquire(limiter::PreviewClass::Preview)
        .await;
    let (tx, rx) = tokio::sync::mpsc::channel::<CtrlChunk>(super::STREAM_WINDOW);
    tokio::spawn(pump_segment(reader, permit, tx));
    CtrlReply::Stream(CtrlStreamOut { chunks: rx })
}

/// 段取错误 → 稳定词单帧(T12-M2 钉死两可区分词,不得共词):
/// - 段未就绪(InvalidInput,生产中)= `segment_pending`(查看端等待)
/// - 段越界(标记类型,段尽)= `segment_range`(查看端止步)
/// - 会话不存在/已回收 = NOT_FOUND(查看端失败面)
/// - 其余 = source_offline(查看端 503 面原料,M5:分类先于重试/跳段)
fn segment_err_response(e: &io::Error) -> CtrlResponse {
    if is_segment_range(e) {
        return err_response("segment_range", format!("段越界(止步): {e}"));
    }
    match e.kind() {
        io::ErrorKind::NotFound => err_response(
            "NOT_FOUND",
            format!("转码会话不存在或已回收(请重新 open): {e}"),
        ),
        io::ErrorKind::InvalidInput => {
            err_response("segment_pending", format!("段未就绪(等待): {e}"))
        }
        _ => err_response("source_offline", format!("段读取失败: {e}")),
    }
}

/// 段泵:逐 [`super::READ_CHUNK`] 块读后端段流 → 有界通道(Sender 不出
/// 任务——EOF/读尽 = 通道关 = DataEnd;泵亡即 send 败自收,令牌随 permit
/// 落放;读败 = source_offline 错误块收尾,不再有 DataEnd)。
async fn pump_segment(
    mut reader: Box<dyn io::Read + Send>,
    _permit: limiter::PreviewPermit,
    tx: tokio::sync::mpsc::Sender<CtrlChunk>,
) {
    loop {
        let read = tokio::task::spawn_blocking(move || {
            let mut buf = vec![0u8; super::READ_CHUNK];
            let n = reader.read(&mut buf)?;
            buf.truncate(n);
            Ok::<_, io::Error>((buf, reader))
        })
        .await;
        match read {
            // EOF:段尽,空余零帧(读面随臂落放——段流读到头即终结)
            Ok(Ok((buf, _r))) if buf.is_empty() => break,
            Ok(Ok((buf, r))) => {
                reader = r;
                if tx.send(Ok(buf)).await.is_err() {
                    return; // 泵亡:流已收,令牌随 permit 落放
                }
            }
            Ok(Err(e)) => {
                let _ = tx
                    .send(Err(mtdss_core::transport::ctrl::CtrlStreamError {
                        code: "source_offline".into(),
                        message: format!("段读取失败: {e}"),
                    }))
                    .await;
                return;
            }
            Err(join) => {
                let _ = tx
                    .send(Err(mtdss_core::transport::ctrl::CtrlStreamError {
                        code: "INTERNAL".into(),
                        message: format!("段读任务收尾失败: {join}"),
                    }))
                    .await;
                return;
            }
        }
    }
}

/// `rendition_cancel` 应答(守卫直通臂已过):会话回收(App 侧转码资源
/// 即释);proto 钉死无独立响应体——空 ok 单帧即成(未知会话幂等成功)。
pub(crate) async fn rendition_cancel_reply(
    state: Arc<DaemonState>,
    r: RenditionCancel,
) -> CtrlReply {
    let mgr = state.rendition_sessions();
    let _ = tokio::task::spawn_blocking(move || mgr.cancel(r.session_id)).await;
    CtrlReply::Frame(CtrlResponse {
        version: mtdss_proto::CTRL_VERSION,
        ok: true,
        body: None,
    })
}

// ---------- 查看端装配(拉段入 T5 缓存 + HLS playlist 合成) ----------

/// 查看端装配错误(brief 面:unsupported → 回退原流+海报;其余 = 失败)。
/// 消费面 = T15(loopback video 路由)。
#[derive(Debug)]
pub(crate) enum AttachError {
    /// 源端无转码后端(稳定词 `unsupported`)。
    Unsupported,
    /// 其余失败(稳定码,文案;源离线/NOT_FOUND 等)。
    Failed(String, String),
}

/// 段获取面(查看端装配的源通道):生产 = ctrl 池往返(远端设备)或
/// 进程内直驱源端分发缝(本机 device;免网络面,与远端同守卫/限流/泵
/// 纪律)。错误归一(稳定码,文案),消费面按稳定词分流(T15-M2/M5:
/// segment_pending → 等待;segment_range → 止步;其余 → 失败)。
/// `Weak` 防 state↔装配引用环(两侧同款——state 经 gate↔server↔装配表
/// 可达本结构,强持即环)。
pub(crate) enum RenditionFetch {
    /// 远端设备:ctrl 池往返。
    Remote {
        state: std::sync::Weak<DaemonState>,
        device: String,
    },
    /// 进程内源端(本机 device):直驱 [`super::handle_ctrl_request_reply`]
    /// (守卫/路由/限流全过同缝,与远端 wire 面同语义)。
    Local(std::sync::Weak<DaemonState>),
}

impl RenditionFetch {
    /// 生产构造(远端设备)。
    pub(crate) fn remote(state: &Arc<DaemonState>, device: &str) -> Self {
        Self::Remote {
            state: Arc::downgrade(state),
            device: device.to_string(),
        }
    }

    /// 生产构造(本机设备;device = "local"/self 语义由调用方判定)。
    pub(crate) fn local(state: &Arc<DaemonState>) -> Self {
        Self::Local(Arc::downgrade(state))
    }

    /// 升级 state(两臂共用;失败 = daemon 已收尾)。
    fn upgrade(&self) -> Result<Arc<DaemonState>, (String, String)> {
        match self {
            Self::Remote { state, .. } | Self::Local(state) => state
                .upgrade()
                .ok_or_else(|| ("INTERNAL".into(), "daemon 已收尾".into())),
        }
    }

    async fn open(
        &self,
        path: &str,
        mtime: i64,
        size: u64,
        start_at_ms: u64,
    ) -> Result<RenditionOpenResponse, (String, String)> {
        let req = RenditionOpenRequest {
            path: path.to_string(),
            mtime,
            size: size as i64,
            target: mtdss_proto::ctrl::RenditionTarget::Hls as i32,
            start_at_ms,
        };
        match self {
            Self::Remote { state, device } => {
                let state = state
                    .upgrade()
                    .ok_or_else(|| ("INTERNAL".into(), "daemon 已收尾".into()))?;
                state
                    .ctrl()
                    .preview_rendition_open(&state, device, req)
                    .await
                    .map_err(|e| e.stable())
            }
            Self::Local(_) => {
                let state = self.upgrade()?;
                let creq = CtrlRequest {
                    version: mtdss_proto::CTRL_VERSION,
                    body: Some(ctrl_request::Body::RenditionOpen(req)),
                };
                match super::handle_ctrl_request_reply(state, creq).await {
                    CtrlReply::Frame(resp) => match resp.body {
                        Some(ctrl_response::Body::RenditionOpen(o)) if resp.ok => Ok(o),
                        _ => Err(frame_error(&resp)
                            .unwrap_or_else(|| ("INTERNAL".into(), "open 应答形态不符".into()))),
                    },
                    CtrlReply::Stream(_) => Err(("INTERNAL".into(), "open 应答不应流式".into())),
                }
            }
        }
    }

    async fn segment(&self, session_id: u64, index: u32) -> Result<Vec<u8>, (String, String)> {
        match self {
            Self::Remote { state, device } => {
                let state = state
                    .upgrade()
                    .ok_or_else(|| ("INTERNAL".into(), "daemon 已收尾".into()))?;
                match state
                    .ctrl()
                    .preview_rendition_segment(&state, device, session_id, index)
                    .await
                {
                    Ok(mtdss_core::transport::ctrl::CtrlStreamOutcome::Stream(s)) => {
                        drain_stream(s).await
                    }
                    Ok(mtdss_core::transport::ctrl::CtrlStreamOutcome::Frame(resp)) => {
                        Err(frame_error(&resp)
                            .unwrap_or_else(|| ("INTERNAL".into(), "段应答形态不符".into())))
                    }
                    Err(e) => Err(e.stable()),
                }
            }
            Self::Local(_) => {
                let state = self.upgrade()?;
                let creq = CtrlRequest {
                    version: mtdss_proto::CTRL_VERSION,
                    body: Some(ctrl_request::Body::RenditionSegment(
                        RenditionSegmentRequest { session_id, index },
                    )),
                };
                match super::handle_ctrl_request_reply(state, creq).await {
                    CtrlReply::Stream(out) => drain_chunks(out.chunks).await,
                    CtrlReply::Frame(resp) => Err(frame_error(&resp)
                        .unwrap_or_else(|| ("INTERNAL".into(), "段应答形态不符".into()))),
                }
            }
        }
    }

    async fn cancel(&self, session_id: u64) -> Result<(), (String, String)> {
        match self {
            Self::Remote { state, device } => {
                let state = state
                    .upgrade()
                    .ok_or_else(|| ("INTERNAL".into(), "daemon 已收尾".into()))?;
                state
                    .ctrl()
                    .preview_rendition_cancel(&state, device, session_id)
                    .await
                    .map_err(|e| e.stable())
            }
            Self::Local(_) => {
                let state = self.upgrade()?;
                let creq = CtrlRequest {
                    version: mtdss_proto::CTRL_VERSION,
                    body: Some(ctrl_request::Body::RenditionCancel(RenditionCancel {
                        session_id,
                    })),
                };
                match super::handle_ctrl_request_reply(state, creq).await {
                    CtrlReply::Frame(resp) if resp.ok => Ok(()),
                    CtrlReply::Frame(resp) => Err(frame_error(&resp)
                        .unwrap_or_else(|| ("INTERNAL".into(), "取消应答形态不符".into()))),
                    CtrlReply::Stream(_) => Err(("INTERNAL".into(), "取消应答不应流式".into())),
                }
            }
        }
    }
}

/// 单帧错误投影:CtrlError 体 = (稳定码, 文案);其余 None。
fn frame_error(resp: &CtrlResponse) -> Option<(String, String)> {
    resp.body.as_ref().and_then(|b| match b {
        ctrl_response::Body::Error(e) => Some((e.code.clone(), e.message.clone())),
        _ => None,
    })
}

/// 流式收齐(Local 面:通道项即生产块)。
async fn drain_chunks(
    mut rx: tokio::sync::mpsc::Receiver<CtrlChunk>,
) -> Result<Vec<u8>, (String, String)> {
    let mut bytes = Vec::new();
    while let Some(item) = rx.recv().await {
        match item {
            Ok(c) => bytes.extend_from_slice(&c),
            Err(e) => return Err((e.code, e.message)),
        }
    }
    Ok(bytes)
}

/// 流式收齐(生产 Remote 面:T2 泵手柄逐块消费,段字节 ~MB 级整收即
/// 落缓存,无需透传流)。
async fn drain_stream(
    mut s: mtdss_core::transport::ctrl::CtrlStreamResponse,
) -> Result<Vec<u8>, (String, String)> {
    let mut bytes = Vec::new();
    while let Some(item) = s.next_chunk().await {
        match item {
            Ok(c) if c.is_empty() => continue, // 空块零语义
            Ok(c) => bytes.extend_from_slice(&c),
            Err(e) => return Err((e.code, e.message)),
        }
    }
    Ok(bytes)
}

/// 段缓存键(回环 handle 铸的字母数字前缀 + 段序;T5 valid_handle 只收
/// 字母数字,handle = 32 hex 恒过;init 段 0 同面)。
fn seg_cache_key(prefix: &str, index: u32) -> String {
    format!("{prefix}vseg{index}")
}

// ---------- 段请求面错误分类与失败形(T15) ----------

/// 段请求错误面(T15-M5:**分类先于重试/跳段逻辑**)——按源端稳定词投影,
/// 勿吞为段尽。
#[derive(Debug)]
enum SegFace {
    /// `segment_pending`:生产中,同段稍后重试可达(等待窗原料)。
    Pending,
    /// `segment_range`:段尽,止步(勿等待勿重试)。
    Range,
    /// 其余(source_offline/会话回收/INTERNAL)= 失败面(503 原料)。
    Failed(String, String),
}

/// 段请求失败形(seg 路由 → HTTP 面映射)。
#[derive(Debug)]
pub(crate) enum SegFail {
    /// 有界等待窗烧尽仍段未就绪 → 503(Review#1)。
    Timeout,
    /// 段越界(止步)→ 404(T15-M2 止步词)。
    Range,
    /// 失败面(源离线/会话回收/落盘)→ 503 + 稳定词。
    Failed(String, String),
}

/// 段请求错误分类(M2 稳定词先行;未识词一律失败面——勿静默归段尽)。
fn classify_seg_face(err: &(String, String)) -> SegFace {
    match err.0.as_str() {
        "segment_pending" => SegFace::Pending,
        "segment_range" => SegFace::Range,
        _ => SegFace::Failed(err.0.clone(), err.1.clone()),
    }
}

/// 段缓存件在盘查询(T12-M3:**消费时过存在性**)——T5 记账在场 ≠ 盘上
/// 在(LRU 驱逐/外力删除面);盘上缺席 = None,调用方回源重拉。
fn cached_segment(
    cache: &super::cache::PreviewDiskCache,
    prefix: &str,
    index: u32,
) -> Option<PathBuf> {
    cache
        .path_for(&seg_cache_key(prefix, index))
        .filter(|p| p.exists())
}

// ---------- 等待窗常量(T15;brief 钉死 10s) ----------

/// 跳段有界等待窗(brief:「有界等待 10s→503」;常量锚,改值须过测试)。
pub(crate) const RENDITION_SEG_WAIT: Duration = Duration::from_secs(10);
/// 等待窗内段探测间隔(每轮一次完整 ctrl 往返;4s 段长生产节拍下 500ms
/// 足密)。等待环全走 tokio 计时——测试 `start_paused` 拨钟即烧窗,零真实
/// 等待(T12 Review#2 注入时钟的 async 面:tokio 暂停钟即注入钟)。
pub(crate) const RENDITION_SEG_POLL: Duration = Duration::from_millis(500);
/// 单次前缀推进窗(段数):playlist 每次刷新至多新拉 8 段(≈32s @4s 段长)
/// ——首刷不整拉长视频(2h 片 = 千段级往返会拖死首字节),生产追平由后续
/// 刷新/跳段继续;「已缓存段列出」语义只关缓存,窗口只影响推进步长。
const RENDITION_PULL_WINDOW: u32 = 16;
/// 远跳重开触发点(T15b:等待窗**过半**仍未达 = 目标段在生产前沿远端,
/// v1 死等 ≈ 整段转码时长必烧尽 → cancel+携 start_at 重开换道)。常量锚
/// (×2 == 等待窗,测试钉面),改值须过测试。
pub(crate) const RENDITION_SEG_REOPEN_AT: Duration = Duration::from_secs(5);
/// 段等待超界(503)的「重试」语义稳定词(T21 消费):超界响应体必携本
/// 词,响应头携 `Retry-After: {SEG_RETRY_AFTER_SECS}`(秒)。语义 = 同段
/// 稍后重试可达(远跳重开已在等待路径自动发生,本词面向重开后余窗仍尽
/// 的面);稳定词勿与错误码共词面(独立词表,T21 词面匹配)。
pub(crate) const SEG_RETRY_WORD: &str = "seg_retry";
/// `Retry-After` 头值(秒;对齐探测间隔量级,播放端重试节律原料)。
pub(crate) const SEG_RETRY_AFTER_SECS: u64 = 2;

/// 会话态(锁内短临界区零 await;远跳重开可整态置换)。
struct SessionState {
    session_id: u64,
    segment_ms: u32,
    mime: String,
    /// 重开锚(T15b):当前会话内容段 1 对应的查看端段号(attach = 1;
    /// 远跳重开 = 目标段号——段索引空间沿用,playlist 自锚重建,不重置)。
    origin: u32,
    /// 已拉段数(**含 init 段**:pulled == 1 = 仅 init;≥ 2 = init +
    /// `pulled-1` 个自锚起的连续内容段——内容入列以 init 在场为门,旧形
    /// 同语义;跳段乱序到达不推账,playlist 段序无洞纪律)。
    pulled: u32,
    /// 会话代次(远跳重开判重:并发等待者各自携带入窗代次,过半触发时代
    /// 次已翻 = 他请求已重开,勿再 cancel+open)。
    gen: u64,
    /// init 段缓存代次(== gen 时盘上 vseg0 属当前会话;远跳重开 gen 翻
    /// 转即作废旧 init——重开换管线 = 旧 moov 作废,下次拉取以当前会话
    /// init 覆写缓存件,MAP 面字节恒当前。缓存无删除 API,代次判据替代)。
    init_gen: u64,
    /// 源端段尽(field 修:拉段遇段越界 = 源端自然完毕——playlist 据此
    /// 封 ENDLIST,时长锁定全长 + 播放端结束态)。
    completed: bool,
    /// 源总时长毫秒(r29:open 应答携带;0 = 未知 → EVENT 生长形)。
    duration_ms: u64,
}

/// 查看端转码装配(每视频一份,`Arc` 共享于并发请求):持有源端会话、
/// 重开锚/前缀推进账与跳段等待参数;段字节落 T5 缓存(`{home}/preview-tmp`,
/// 键 = `{handle}vseg{i}`,prefix = 回环 handle;段键 = **查看端段号**
/// ——远跳重开换会话不换键空间,旧会话已拉段照常命中)。并发面:会话态
/// 为 std 锁短临界区(锁内零 await);段取/等待/重开环在锁外并发——同段
/// 并发双拉各自请求(T5 store 同键替换、字节同源幂等;hls.js 单段单请求),
/// 重开经 tokio 串行门 + 代次判重收敛为一枚新会话(见
/// [`ViewerRendition::reopen_far`])。
pub(crate) struct ViewerRendition {
    fetch: RenditionFetch,
    /// 会话态(远跳重开可换)。
    state: Mutex<SessionState>,
    /// 重开串行门(tokio 锁:重开含 cancel+open 两个 await 往返,不占
    /// std 锁;并发同目标的过半触发在锁内代次判重收敛)。
    reopen_gate: tokio::sync::Mutex<()>,
    /// 重开原料(attach 入参留底;重开 = 同源同件携新 start_at 再 open)。
    path: String,
    mtime: i64,
    size: u64,
}

impl ViewerRendition {
    /// 装配:rendition_open(start_at = 0 从头)→ 会话元信息。
    /// Err(Unsupported) = 源端无后端 → 调用方回退原流+海报;Err(Failed)
    /// = 其余失败。
    pub(crate) async fn attach(
        fetch: RenditionFetch,
        path: &str,
        mtime: i64,
        size: u64,
    ) -> Result<Self, AttachError> {
        let resp = fetch
            .open(path, mtime, size, 0)
            .await
            .map_err(|(code, msg)| {
                if code == "unsupported" {
                    AttachError::Unsupported
                } else {
                    AttachError::Failed(code, msg)
                }
            })?;
        Ok(Self {
            fetch,
            state: Mutex::new(SessionState {
            completed: false,
                duration_ms: resp.duration_ms,
                session_id: resp.session_id,
                segment_ms: resp.segment_ms,
                mime: resp.mime,
                origin: 1,
                pulled: 0,
                gen: 0,
                init_gen: u64::MAX, // 本 handle 尚无本会话 init(拉取即覆写)
            }),
            reopen_gate: tokio::sync::Mutex::new(()),
            path: path.to_string(),
            mtime,
            size,
        })
    }

    /// 源端会话 id(测试缝:背会话拆台面;远跳重开后 = 当前会话)。
    #[cfg(test)]
    pub(crate) fn session_id(&self) -> u64 {
        self.state.lock().unwrap().session_id
    }

    /// 重开锚(测试缝:origin 毒化钉面——T15b 评审 Important #1)。
    #[cfg(test)]
    pub(crate) fn origin(&self) -> u32 {
        self.state.lock().unwrap().origin
    }

    /// 段 MIME(open 应答透传;loopback 段应答 Content-Type 面)。
    pub(crate) fn mime(&self) -> String {
        self.state.lock().unwrap().mime.clone()
    }

    /// 会话代次快照(等待环入窗携带;重开判重原料)。
    fn gen_snapshot(&self) -> u64 {
        self.state.lock().unwrap().gen
    }

    /// 盘上 init 段是否属当前会话(代次判据;false = 重开换管线后的作废
    /// 面,拉取路径须以当前会话 init 覆写)。
    fn init_current(&self) -> bool {
        let st = self.state.lock().unwrap();
        st.init_gen == st.gen
    }

    /// 盘上 init 段标记为当前会话(拉取/覆写后;代次取当前快照)。
    fn mark_init_current(&self) {
        let mut st = self.state.lock().unwrap();
        st.init_gen = st.gen;
    }

    /// 查看端段号 → (会话 id, 会话段号) 映射(锁内快照;T15b 索引空间
    /// 单点):0 = init(两域恒 0);≥ origin = 目标域(index - origin + 1);
    /// 0 < index < origin = **重开洞域**(旧会话已 cancel、新会话不产)→
    /// None(调用方投影止步面)。
    fn map_segment(&self, index: u32) -> Option<(u64, u32)> {
        let st = self.state.lock().unwrap();
        if index == 0 {
            return Some((st.session_id, 0));
        }
        if index >= st.origin {
            Some((st.session_id, index - st.origin + 1))
        } else {
            None
        }
    }

    /// 前缀推进(账面):段 `idx`(查看端段号)刚落盘,恰为前缀下一枚即
    /// 推进——init(0)起步(pulled 0→1);内容段 = 锚域连续下一枚
    /// (idx == origin + pulled - 1,账含 init;**内容入列以 init 在场为
    /// 门**,pulled == 0 时内容先行落盘不推账)。跳段乱序到达/代次已翻
    /// (并发重开把本段划出当前前缀空间)不推——playlist 段序无洞纪律。
    fn advance_pulled(&self, idx: u32) {
        let mut st = self.state.lock().unwrap();
        if idx == 0 {
            if st.pulled == 0 {
                st.pulled = 1;
            }
            return;
        }
        if st.pulled >= 1 && st.origin + st.pulled - 1 == idx {
            st.pulled += 1;
        }
    }

    fn pulled_now(&self) -> u32 {
        self.state.lock().unwrap().pulled
    }

    /// 落段入 T5 缓存(store 同键替换;失败 = source_offline 失败面)。
    async fn store_segment(
        &self,
        cache: &super::cache::PreviewDiskCache,
        prefix: &str,
        index: u32,
        bytes: &[u8],
    ) -> Result<(), (String, String)> {
        cache
            .store(
                &seg_cache_key(prefix, index),
                &mut io::Cursor::new(bytes),
            )
            .map(|_| ())
            .map_err(|e| ("source_offline".into(), format!("段缓存落盘失败: {e}")))
    }

    /// 前缀顺序推进(playlist 刷新臂):init 前奏(未在盘或属旧会话代次
    /// ——远跳重开换管线 = 旧 init 作废——即拉当前会话 init 覆写,MAP 面
    /// 字节恒当前)→ 自当前前缀端(origin + pulled - 1)起逐内容段拉(缓
    /// 存件在盘即免拉推进,T12-M3 回源/跳段已拉两 face 同短路),至首个止
    /// 步面(段未就绪=生产未至/段越界=段尽)或推进窗尽。**失败面即上抛,
    /// 勿吞为段尽**(T12-M5:source_offline 分类先于一切拉取序逻辑——中
    /// 途离线的半截 playlist 对播放端是无提示死路,503 面才可重试)。段号
    /// 全按查看端索引空间(映射归 [`Self::map_segment`]——远跳重开后自然
    /// 落新会话锚域);「取消当前拉取序」语义:本环为按需短程(窗 8 段
    /// 封顶),跳段臂直接乱序直取目标段即改写拉取序,无需中断在途环。
    /// 返回 = 已拉段数(含 init,旧形同语义)。
    pub(crate) async fn pull_segments(
        &self,
        cache: &super::cache::PreviewDiskCache,
        prefix: &str,
    ) -> Result<u32, (String, String)> {
        // init 前奏:未在盘或代次已翻(重开作废旧 moov)即拉当前会话 init
        if !self.init_current() || cached_segment(cache, prefix, 0).is_none() {
            let sid = self.state.lock().unwrap().session_id;
            match self.fetch.segment(sid, 0).await {
                Ok(bytes) => {
                    self.store_segment(cache, prefix, 0, &bytes).await?;
                    self.mark_init_current();
                    self.advance_pulled(0);
                }
                Err(face) => match classify_seg_face(&face) {
                    SegFace::Pending | SegFace::Range => return Ok(self.pulled_now()),
                    SegFace::Failed(c, m) => return Err((c, m)), // M5:失败勿吞
                },
            }
        }
        for _ in 0..RENDITION_PULL_WINDOW {
            let (sid, next, seg) = {
                let st = self.state.lock().unwrap();
                // 前缀下一枚:init 起步(0)后按锚域内容段推进(账含 init)
                let next = if st.pulled == 0 { 0 } else { st.origin + st.pulled - 1 };
                let seg = if next == 0 { 0 } else { next - st.origin + 1 };
                (st.session_id, next, seg)
            };
            if cached_segment(cache, prefix, next).is_some() {
                self.advance_pulled(next);
                continue;
            }
            match self.fetch.segment(sid, seg).await {
                Ok(bytes) => {
                    self.store_segment(cache, prefix, next, &bytes).await?;
                    self.advance_pulled(next);
                }
                Err(face) => match classify_seg_face(&face) {
                    SegFace::Pending => break, // 止步:生产未至
                    // field 修:段越界 = 源端自然完毕(0x0C eof 后引擎改答
                    // 越界)——记段尽,playlist 封 ENDLIST
                    SegFace::Range => {
                        self.state.lock().unwrap().completed = true;
                        break;
                    }
                    SegFace::Failed(c, m) => return Err((c, m)), // M5:失败勿吞
                },
            }
        }
        Ok(self.pulled_now())
    }

    /// 跳段有界等待(T15;T15b 远跳重开):目标段直取,段未就绪按窗内轮
    /// 询(全 tokio 计时,暂停钟可测);**窗过半([`RENDITION_SEG_REOPEN_AT`])
    /// 仍未达 = 远跳重开**(spec §5.1「以 start_at 就近关键帧重启转码」:
    /// cancel 当前会话 + 携 start_at = 目标段×段长 重开 + 余窗续等,总界
    /// 10s 不变)——v1 死等整段转码时长的换道面。**init 段(0)除外**:
    /// seg/0 是 MAP 面(每次 playlist 载入即请求),慢产 init 重开 = 携
    /// start_at = 0×段长 = 0 重开,origin 落 0 毒化三映射不变量
    /// (map_segment 的 0 分支遮蔽锚 / advance_pulled 停摆 / playlist
    /// MAP+EXTINF 双列 seg/0)→ 永久空播无自愈——慢 init 走烧窗
    /// 503+seg_retry 诚实面。每等待至多重开一枚(本地旗 + 代次判重双
    /// 保险,见 [`Self::reopen_far`]);越界/失败面即时上抛(M5:分类先于
    /// 等待,失败不入窗烧时);重开洞域段号 = 止步。
    async fn fetch_with_wait(&self, index: u32) -> Result<Vec<u8>, SegFace> {
        let started = tokio::time::Instant::now();
        let deadline = started + RENDITION_SEG_WAIT;
        let mut gen = self.gen_snapshot();
        let mut reopened = false;
        loop {
            let Some((sid, seg)) = self.map_segment(index) else {
                return Err(SegFace::Range); // 重开洞域:旧会话已弃,止步
            };
            match self.fetch.segment(sid, seg).await {
                Ok(bytes) => return Ok(bytes),
                Err(face) => match classify_seg_face(&face) {
                    SegFace::Range => return Err(SegFace::Range),
                    SegFace::Failed(c, m) => return Err(SegFace::Failed(c, m)),
                    SegFace::Pending => {
                        let now = tokio::time::Instant::now();
                        if now >= deadline {
                            return Err(SegFace::Pending); // 窗尽 → Timeout 面
                        }
                        if !reopened
                            && index != 0
                            && now - started >= RENDITION_SEG_REOPEN_AT
                        {
                            self.reopen_far(index, gen).await;
                            gen = self.gen_snapshot();
                            reopened = true;
                        }
                        tokio::time::sleep(RENDITION_SEG_POLL).await;
                    }
                },
            }
        }
    }

    /// 远跳重开(T15b,brief 钉死顺序 **cancel 先行**):远目标段旧会话窗
    /// 内必达不了,先 cancel 释放 App 侧转码管线,再携 `start_at_ms =
    /// 目标段×段长` 重开(App 侧就近关键帧重产,段索引空间沿用——新会话
    /// 内容段 1 即查看端目标段,锚在 [`SessionState::origin`]);重开原料
    /// = attach 留底的 path/mtime/size。串行门 + 代次判重:并发等待者同
    /// 触发时后到者见代次已翻即让(一枚新会话收敛面);重开失败(源端离
    /// 线/unsupported)= 旧会话已 cancel 的死面,代次照翻封闸(其余等待
    /// 者不再重复 cancel+open;后续段读 NotFound = 503 诚实面)。
    async fn reopen_far(&self, target: u32, observed_gen: u64) {
        let _gate = self.reopen_gate.lock().await;
        {
            let st = self.state.lock().unwrap();
            if st.gen != observed_gen {
                return; // 并发请求已重开:让面(勿再 cancel+open)
            }
        }
        let (old_sid, segment_ms) = {
            let st = self.state.lock().unwrap();
            (st.session_id, st.segment_ms)
        };
        let start_at_ms = target as u64 * segment_ms as u64;
        tracing::info!(target, start_at_ms, "段等待过半未达,远跳重开转码会话");
        let _ = self.fetch.cancel(old_sid).await;
        match self
            .fetch
            .open(&self.path, self.mtime, self.size, start_at_ms)
            .await
        {
            Ok(resp) => {
                let mut st = self.state.lock().unwrap();
                st.session_id = resp.session_id;
                st.segment_ms = resp.segment_ms;
                st.mime = resp.mime;
                st.origin = target;
                st.pulled = 0;
                st.gen += 1;
            }
            Err((code, msg)) => {
                tracing::warn!(%code, %msg, target, "远跳重开失败(源端面),等待按窗尽收口");
                self.state.lock().unwrap().gen += 1;
            }
        }
    }

    /// 段请求保证面(seg 路由):缓存件在盘即读盘直出(T12-M3 命中面:
    /// 前缀内/跳段已拉同面;读盘竞窗失败=按缺回源;**init 例外**——重开
    /// 后代次已翻的旧 init 不命中,回源当前会话覆写);缺 = 跳段直取目标
    /// 段(有界等待,窗过半远跳重开——T15b),到手落缓存,恰为前缀下一
    /// 枚时推账(段序无洞;init 落盘标记当前会话代次)。
    pub(crate) async fn ensure_segment(
        &self,
        cache: &super::cache::PreviewDiskCache,
        prefix: &str,
        index: u32,
    ) -> Result<Vec<u8>, SegFail> {
        let init_stale = index == 0 && !self.init_current();
        if !init_stale {
            if let Some(p) = cached_segment(cache, prefix, index) {
                match tokio::fs::read(&p).await {
                    Ok(bytes) => return Ok(bytes),
                    Err(e) => {
                        tracing::warn!(%e, path = %p.display(), "段缓存件读盘失败,回源重拉");
                    }
                }
            }
        }
        match self.fetch_with_wait(index).await {
            Ok(bytes) => {
                self.store_segment(cache, prefix, index, &bytes)
                    .await
                    .map_err(|(c, m)| SegFail::Failed(c, m))?;
                if index == 0 {
                    self.mark_init_current();
                }
                self.advance_pulled(index);
                Ok(bytes)
            }
            Err(SegFace::Pending) => Err(SegFail::Timeout),
            Err(SegFace::Range) => Err(SegFail::Range),
            Err(SegFace::Failed(c, m)) => Err(SegFail::Failed(c, m)),
        }
    }

    /// 动态 HLS playlist(已拉段列出,未拉段不列;`#EXT-X-VERSION:7`/
    /// fMP4/`#EXT-X-MAP`)。**段 URI 恒绝对+携 `?t=` 查询令牌**(T21 fix
    /// round 1 Critical #1:相对 URI 经 hls.js 按 RFC 3986 对 playlist URL
    /// 解析会**丢查询串**——段请求即无凭据,auth 中间件恒 401(先于 410/
    /// 503),退避环不触,fatal 误落 HEVC 引导降级链;单点修复于 playlist
    /// 生成面,全客户端受益,消费端零改动)。`seg_prefix` = 回环段路由绝对
    /// 前缀(`http://127.0.0.1:{port}/f/{h}/video/seg`,路由面拼装——SrvCtx
    /// 持 port/token);`token` = 回环口查询令牌。
    /// 终未到不写 `#EXT-X-ENDLIST`(EVENT 形;前缀随推进窗/跳段补齐生长)。
    /// pulled ≤ 1(仅 attach/init)= 纯头形;内容段自重开锚 origin 列
    /// (`origin..origin + pulled - 1`,段索引空间沿用——attach origin=1 =
    /// 旧行为;远跳重开 = playlist 按新会话重建,旧会话段不列,事件形
    /// playlist 的重建面对播放端 = 用户 seek 手势后的重定位,T21 消费面)。
    pub(crate) fn playlist_m3u8(&self, seg_prefix: &str, token: &str) -> String {
        let (origin, pulled, segment_ms, completed, duration_ms) = {
            let st = self.state.lock().unwrap();
            (st.origin, st.pulled, st.segment_ms, st.completed, st.duration_ms)
        };
        let target = segment_ms.div_ceil(1000).max(1);
        let secs = segment_ms as f64 / 1000.0;
        let mut s = format!(
            "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-PLAYLIST-TYPE:EVENT\n#EXT-X-TARGETDURATION:{target}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-MAP:URI=\"{seg_prefix}/0.m4s?t={token}\"\n"
        );
        if duration_ms > 0 {
            // r29 VOD 形:源总时长已知 → 全部段一次性列出 + ENDLIST——
            // 时长自第一帧即全长、整条可 seek;未产段由段路由的有界等
            // 待/远跳重开兜(hls.js 按需请求)。重开锚(origin>1)时自
            // 锚列至片尾(查看端索引空间沿用)。
            let total = u32::try_from(duration_ms.div_ceil(segment_ms as u64))
                .unwrap_or(u32::MAX);
            for i in origin..=total {
            s.push_str(&format!("#EXTINF:{secs:.3},\n{seg_prefix}/{i}.m4s?t={token}\n"));
            }
            s.push_str("#EXT-X-ENDLIST\n");
            return s;
        }
        if pulled >= 2 {
            for i in origin..origin + pulled - 1 {
                s.push_str(&format!("#EXTINF:{secs:.3},\n{seg_prefix}/{i}.m4s?t={token}\n"));
            }
        }
        if completed {
            s.push_str("#EXT-X-ENDLIST\n");
        }
        s
    }

    /// 段缓存件在盘路径(T12-M3 消费时过存在性;测试缝——生产消费面 =
    /// ensure_segment 的字节保证臂)。
    #[cfg(test)]
    pub(crate) fn segment_path(
        &self,
        cache: &super::cache::PreviewDiskCache,
        key_prefix: &str,
        index: u32,
    ) -> Option<PathBuf> {
        cached_segment(cache, key_prefix, index)
    }

    /// 拆卸:cancel 源端会话(App 侧转码资源即释;远跳重开后 = 当前会话)。
    /// 幂等(未知会话照成)。
    pub(crate) async fn detach(&self) {
        let sid = self.state.lock().unwrap().session_id;
        let _ = self.fetch.cancel(sid).await;
    }
}

/// 视频装配表类型(V4-1 T15):回环 handle → 查看端装配。全 daemon 单份
/// (挂在 [`crate::preview::loopback::PreviewGate`],路由面与 IPC
/// preview_close 关闭面共享;消费面 = loopback video 路由惰性 attach,
/// close/路由 410 面即摘 + 后台 detach——源端会话随 handle 释放,spec
/// §7;UI 崩溃漏 close 的残项由源端 60s 空闲回收兜底)。
pub(crate) type VideoSessions =
    Arc<Mutex<std::collections::HashMap<String, Arc<ViewerRendition>>>>;

// ---------- 测试后端(伪转码器)+ 测试面 ----------

/// 伪转码器(cfg(test)):init + 2 内容段(共 3 个可拉段)已知字节,
/// 越界 Err(err_segment_range,T15-M2 止步词);cancel 计数可断言(brief
/// 「3 段已知字节」测试面:open→seg0..2 逐字节;越界→Err;cancel 后
/// seg→Err;60s 回收)。
#[cfg(test)]
pub(crate) struct PseudoTranscoder {
    cancels: std::sync::atomic::AtomicU32,
    /// r29:源总时长(open 应答携;缺省 0 = EVENT 生长形)。
    duration_ms: std::sync::atomic::AtomicU64,
}

#[cfg(test)]
impl PseudoTranscoder {
    /// init 段已知字节(段 0;EXT-X-MAP 面)。pub(crate):loopback 用例
    /// 同语消费面(段字节断言)。
    pub(crate) fn init_bytes() -> Vec<u8> {
        b"PSEUDO-FMP4-INIT-HEADER-0".to_vec()
    }

    /// 内容段已知字节(段 1..=2;长度随序可复算,泵多块面不在此造——
    /// 段字节远小于 READ_CHUNK,单块到齐)。
    pub(crate) fn segment_bytes(index: u32) -> Vec<u8> {
        format!("PSEUDO-CONTENT-{index:04}<")
            .repeat(24 + index as usize * 8)
            .into_bytes()
    }

    fn cancels(&self) -> u32 {
        self.cancels.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
impl Transcoder for PseudoTranscoder {
    fn duration_ms(&self) -> u64 {
        self.duration_ms.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn segment(&self, index: u32) -> io::Result<Box<dyn io::Read + Send>> {
        match index {
            0 => Ok(Box::new(io::Cursor::new(Self::init_bytes()))),
            1..=2 => Ok(Box::new(io::Cursor::new(Self::segment_bytes(index)))),
            _ => Err(err_segment_range(index)),
        }
    }

    fn cancel(&self) {
        self.cancels
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// 行为可调伪转码器(cfg(test);T15 面):init 段恒真件,内容段行为按
/// 模式;调用序记录(跳段乱序直取断言面)+ cancel 计数。pub(crate):
/// loopback 用例同语消费面(工厂安装 + 调用序断言)。
#[cfg(test)]
enum FakeBehavior {
    /// 标准三段(index 1/2 产出,越界 range)。
    Standard,
    /// 恒段未就绪(等待面;10s 烧窗用)。
    Pending,
    /// 段 1 先 pending 数轮再产出(等待窗内成功面;剩余 pending 次数)。
    Deferred(std::sync::atomic::AtomicU32),
    /// init 后恒读败(首段成功中途失败;M5 中途失败勿吞用)。
    ContentFailing,
    /// start_at 敏感(T15b 远跳重开测试面):start_at=0 开的会话内容段恒
    /// 未就绪(v1 死等面:顺序推进追不上远端目标段);start_at>0 开的会话
    /// (重开面)产「SEEKED」标记字节段 1..=2,越界止步——重开后目标段即
    /// 达的重开收益钉面。
    StartAtGated,
    /// init 恒未就绪(慢产 init 面;T15b 评审 Important #1:seg/0 请求
    /// 不触发重开——start_at=0×段长=0 重开即 origin=0 毒化三映射不变量,
    /// 慢 init 走烧窗 503+seg_retry 诚实面;内容段落标准臂)。
    InitPendingForever,
}

#[cfg(test)]
pub(crate) struct FakeTranscoder {
    behavior: FakeBehavior,
    /// 本会话开启时的 start_at_ms(工厂入参留底;StartAtGated 判据)。
    open_start_at: u64,
    calls: Mutex<Vec<u32>>,
    cancels: std::sync::atomic::AtomicU32,
}

#[cfg(test)]
impl FakeTranscoder {
    /// 标准三段。
    pub(crate) fn standard() -> Self {
        Self {
            behavior: FakeBehavior::Standard,
            open_start_at: 0,
            calls: Mutex::new(Vec::new()),
            cancels: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// 恒段未就绪(等待面)。
    pub(crate) fn pending() -> Self {
        Self {
            behavior: FakeBehavior::Pending,
            open_start_at: 0,
            calls: Mutex::new(Vec::new()),
            cancels: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// 段 1 先 `n` 轮 pending 再产出。
    pub(crate) fn deferred(n: u32) -> Self {
        Self {
            behavior: FakeBehavior::Deferred(std::sync::atomic::AtomicU32::new(n)),
            open_start_at: 0,
            calls: Mutex::new(Vec::new()),
            cancels: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// init 后恒读败(中途失败面)。
    pub(crate) fn content_failing() -> Self {
        Self {
            behavior: FakeBehavior::ContentFailing,
            open_start_at: 0,
            calls: Mutex::new(Vec::new()),
            cancels: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// start_at 敏感(T15b):重开收益面,见 [`FakeBehavior::StartAtGated`]。
    pub(crate) fn gated(start_at_ms: u64) -> Self {
        Self {
            behavior: FakeBehavior::StartAtGated,
            open_start_at: start_at_ms,
            calls: Mutex::new(Vec::new()),
            cancels: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// init 恒未就绪(慢产 init 面;内容段照常标准产出),见
    /// [`FakeBehavior::InitPendingForever`]。
    pub(crate) fn init_pending() -> Self {
        Self {
            behavior: FakeBehavior::InitPendingForever,
            open_start_at: 0,
            calls: Mutex::new(Vec::new()),
            cancels: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// 重开会话的内容段字节(SEEKED 标记,与从头会话的段字节可区分——
    /// 远跳重开链路的字节证词)。pub(crate):loopback 用例同语消费面。
    pub(crate) fn seeked_bytes(index: u32) -> Vec<u8> {
        format!("PSEUDO-SEEKED-{index:04}<")
            .repeat(24 + index as usize * 8)
            .into_bytes()
    }

    /// 段请求调用序(跳段乱序断言面)。
    pub(crate) fn calls(&self) -> Vec<u32> {
        self.calls.lock().unwrap().clone()
    }

    /// cancel 计数(断言面:远跳重开 = 旧会话回收恰一次)。
    pub(crate) fn cancels(&self) -> u32 {
        self.cancels
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
impl Transcoder for FakeTranscoder {
    fn segment(&self, index: u32) -> io::Result<Box<dyn io::Read + Send>> {
        self.calls.lock().unwrap().push(index);
        // InitPendingForever:init 恒未就绪(T15b 评审 Important #1 测试面;
        // 内容段落标准臂)
        if matches!(self.behavior, FakeBehavior::InitPendingForever) && index == 0 {
            return Err(err_segment_pending(index));
        }
        if index == 0 {
            // StartAtGated 重开会话:init 随管线重产(SEEKED 标记)——
            // 「重开作废旧 init,覆写钉面」的字节证词;其余会话 = 通用 init
            if matches!(self.behavior, FakeBehavior::StartAtGated) && self.open_start_at > 0 {
                return Ok(Box::new(io::Cursor::new(Self::seeked_bytes(0))));
            }
            return Ok(Box::new(io::Cursor::new(PseudoTranscoder::init_bytes())));
        }
        match &self.behavior {
            FakeBehavior::Standard | FakeBehavior::InitPendingForever => match index {
                1..=2 => Ok(Box::new(io::Cursor::new(PseudoTranscoder::segment_bytes(index)))),
                _ => Err(err_segment_range(index)),
            },
            FakeBehavior::Pending => Err(err_segment_pending(index)),
            FakeBehavior::Deferred(remaining) => {
                let prev = remaining.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                if prev > 0 {
                    Err(err_segment_pending(index))
                } else {
                    Ok(Box::new(io::Cursor::new(PseudoTranscoder::segment_bytes(index))))
                }
            }
            FakeBehavior::ContentFailing => {
                Err(io::Error::other(format!("源读失败(伪): {index}")))
            }
            FakeBehavior::StartAtGated => {
                if self.open_start_at == 0 {
                    Err(err_segment_pending(index))
                } else {
                    match index {
                        1..=2 => Ok(Box::new(io::Cursor::new(Self::seeked_bytes(index)))),
                        _ => Err(err_segment_range(index)),
                    }
                }
            }
        }
    }

    fn cancel(&self) {
        self.cancels
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// 伪转码器工厂(测试面):每 open 造一枚,实例收进共享槽(cancel
/// 计数断言面)。vpath/start_at 入参照录不消费(桌面伪器无 App 桥面)。
#[cfg(test)]
fn pseudo_factory() -> (BackendFactory, Arc<Mutex<Vec<Arc<PseudoTranscoder>>>>) {
    test_factory(|| PseudoTranscoder {
        cancels: std::sync::atomic::AtomicU32::new(0),
        duration_ms: std::sync::atomic::AtomicU64::new(0),
    })
}

/// 测试工厂(通用形态,cfg(test)):每 open 造一枚(构造闭包),实例
/// 收进共享槽(行为可调伪转码器的调用序/cancel 断言面);start_at 入参
/// 忽略(记录需求归 [`test_factory_start_at`])。pub(crate):
/// loopback 用例同语消费面。
#[cfg(test)]
pub(crate) fn test_factory<T, F>(make: F) -> (BackendFactory, Arc<Mutex<Vec<Arc<T>>>>)
where
    T: Transcoder + 'static,
    F: Fn() -> T + Send + Sync + 'static,
{
    let made = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&made);
    let factory: BackendFactory = Arc::new(move |_src, _path, _start_at| {
        let t = Arc::new(make());
        sink.lock().unwrap().push(Arc::clone(&t));
        Some(t as Arc<dyn Transcoder>)
    });
    (factory, made)
}

/// 测试工厂产物槽(实例/起点记录;clippy type-alias 面同 [`test_factory`])。
#[cfg(test)]
type TestFactoryStart<T> = (
    BackendFactory,
    Arc<Mutex<Vec<Arc<T>>>>,
    Arc<Mutex<Vec<u64>>>,
);

/// 测试工厂(start_at 记录面,T15b):同 [`test_factory`],另录每次 open
/// 的 start_at_ms,构造闭包收本会话起点(行为可依 start_at 分岔)——
/// 远跳重开断言面(源端收到重开请求且 start_at = 目标段×段长)。
/// pub(crate):loopback 用例同语消费面。
#[cfg(test)]
pub(crate) fn test_factory_start_at<T, F>(make: F) -> TestFactoryStart<T>
where
    T: Transcoder + 'static,
    F: Fn(u64) -> T + Send + Sync + 'static,
{
    let made = Arc::new(Mutex::new(Vec::new()));
    let starts = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&made);
    let starts_sink = Arc::clone(&starts);
    let factory: BackendFactory = Arc::new(move |_src, _path, start_at| {
        let t = Arc::new(make(start_at));
        sink.lock().unwrap().push(Arc::clone(&t));
        starts_sink.lock().unwrap().push(start_at);
        Some(t as Arc<dyn Transcoder>)
    });
    (factory, made, starts)
}

/// 内存源(测试面:FileSource 最小实现)。
#[cfg(test)]
struct MemSource {
    data: Vec<u8>,
}

#[cfg(test)]
impl FileSource for MemSource {
    fn size(&self) -> u64 {
        self.data.len() as u64
    }

    fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
        use std::io::Read as _;
        let mut c = io::Cursor::new(&self.data[..]);
        c.set_position(off);
        c.read(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbox_dest;
    use crate::roots::{RootSource, RootSpec};
    use mtdss_proto::ctrl::ctrl_request::Body as Req;
    use mtdss_proto::ctrl::{CtrlRequest, RenditionTarget};
    use std::io::Read as _;

    /// 测试段 URI 绝对前缀/令牌(与 loopback 路由面拼装同形;playlist_m3u8
    /// 段 URI 绝对化钉面的桩——值本身无关紧要,形状=绝对+?t= 携带)。
    const T_SEG_PREFIX: &str = "http://127.0.0.1:1/f/deadbeefdeadbeefdeadbeefdeadbeef/video/seg";
    const T_TOKEN: &str = "tok0123456789";

    /// 预置会话(preview.rs tests 同款装配面单点)。
    fn seed_session(home: &std::path::Path) {
        let session = crate::session::Session {
            server: "http://127.0.0.1:9".into(),
            jwt: "test".into(),
            refresh: "test".into(),
            device_id: "selfdev".into(),
            ca: None,
        };
        session.save(home).unwrap();
    }

    /// 测试态(独立临时 home;预置会话供路由面)。
    fn test_state() -> Arc<DaemonState> {
        let home = tempfile::tempdir().unwrap();
        seed_session(home.path());
        let (events, _) = tokio::sync::broadcast::channel(16);
        #[cfg(windows)]
        let update = std::sync::Arc::new(crate::update::UpdateService::new(
            crate::update::UpdateConfig::default(),
            events.clone(),
            tokio::sync::watch::channel(false).0,
        ));
        #[cfg(not(windows))]
        let update = ();
        Arc::new(
            DaemonState::load(
                home.path(),
                home.path().join("inbox"),
                None,
                inbox_dest::staging_dir(home.path()),
                false,
                false,
                Default::default(),
                Default::default(),
                crate::ctrl::ctrl_default_tuning(),
                events,
                update,
            )
            .unwrap(),
        )
    }

    /// 测试态 + 保活 TempDir(set_roots 注入授权根/桥根用)。
    fn test_state_with_home() -> (Arc<DaemonState>, tempfile::TempDir) {
        let home = tempfile::tempdir().unwrap();
        seed_session(home.path());
        let (events, _) = tokio::sync::broadcast::channel(16);
        #[cfg(windows)]
        let update = std::sync::Arc::new(crate::update::UpdateService::new(
            crate::update::UpdateConfig::default(),
            events.clone(),
            tokio::sync::watch::channel(false).0,
        ));
        #[cfg(not(windows))]
        let update = ();
        let state = Arc::new(
            DaemonState::load(
                home.path(),
                home.path().join("inbox"),
                None,
                inbox_dest::staging_dir(home.path()),
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

    /// 请求构造小助(三 op;open 带缺省起点 0,起点变体归
    /// [`open_request_start_at`])。
    fn open_request(path: &str, mtime: i64, size: u64) -> CtrlRequest {
        open_request_start_at(path, mtime, size, 0)
    }

    /// open 请求构造(带 start_at_ms;T15b 远跳重开原料)。
    fn open_request_start_at(path: &str, mtime: i64, size: u64, start_at_ms: u64) -> CtrlRequest {
        CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: Some(Req::RenditionOpen(RenditionOpenRequest {
                path: path.to_string(),
                mtime,
                size: size as i64,
                target: RenditionTarget::Hls as i32,
                start_at_ms,
            })),
        }
    }

    fn seg_request(session: u64, index: u32) -> CtrlRequest {
        CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: Some(Req::RenditionSegment(RenditionSegmentRequest {
                session_id: session,
                index,
            })),
        }
    }

    fn cancel_request(session: u64) -> CtrlRequest {
        CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: Some(Req::RenditionCancel(RenditionCancel {
                session_id: session,
            })),
        }
    }

    /// 单帧应答投影 (ok, code)(preview.rs tests 同款)。
    fn frame_code(reply: &CtrlReply) -> Option<(bool, String)> {
        let CtrlReply::Frame(resp) = reply else {
            return None;
        };
        resp.body
            .as_ref()
            .and_then(|b| match b {
                ctrl_response::Body::Error(e) => Some(e.code.clone()),
                _ => None,
            })
            .map(|code| (resp.ok, code))
    }

    /// 单帧投影 (ok, code, message)(路由词面断言面)。
    fn frame_msg(reply: &CtrlReply) -> Option<(bool, String, String)> {
        let CtrlReply::Frame(resp) = reply else {
            return None;
        };
        resp.body.as_ref().and_then(|b| match b {
            ctrl_response::Body::Error(e) => Some((resp.ok, e.code.clone(), e.message.clone())),
            _ => None,
        })
    }

    /// 流收齐(单测面无泵——生产块即 recv 项)。
    async fn drain(mut rx: tokio::sync::mpsc::Receiver<CtrlChunk>) -> Vec<u8> {
        let mut bytes = Vec::new();
        while let Some(item) = rx.recv().await {
            match item {
                Ok(c) => bytes.extend_from_slice(&c),
                Err(e) => panic!("流中失败(用例未预期): {e:?}"),
            }
        }
        bytes
    }

    /// 垫料视频件(4096 零字节;转码面不触其内容,源只过手)。独立
    /// tempdir(不落 home 下——测试态的 home TempDir 随构造函数收尾)。
    fn make_video() -> (tempfile::TempDir, String, u64) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("movie.bin");
        std::fs::write(&path, vec![0u8; 4096]).unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        (dir, path.to_string_lossy().into_owned(), size)
    }

    /// 常量锚(spec §4 v1 形态)。
    #[test]
    fn rendition_config_constants() {
        let c = RenditionConfig::DEFAULT;
        assert_eq!(c.segment_ms, 4000, "段长 4000ms");
        assert_eq!(c.height, 1080, "1080p");
        assert_eq!(c.bitrate_bps, (4_000_000, 6_000_000), "4–6Mbps");
        assert_eq!(c.segment_ms, RenditionConfig::SEGMENT_MS);
        assert_eq!(RENDITION_IDLE_TTL, Duration::from_secs(60), "空闲 60s 回收");
        assert_eq!(SEGMENT_MIME, "video/iso.segment");
    }

    /// e2e 相①②(brief:open→seg0..2 逐字节;seg 越界→Err):分发缝
    /// (handle_ctrl_request_reply)直驱伪转码器全链路——open 单帧
    /// (session/segment_ms/mime)→ 段 0(init)/1/2 流式逐字节 → 越界
    /// index 3 = NOT_FOUND 单帧携「越界」词。
    #[tokio::test]
    async fn open_then_segments_byte_exact_and_out_of_range() {
        let state = test_state();
        state
            .rendition_sessions()
            .install_factory(pseudo_factory().0);
        let (_dir, path, size) = make_video();

        // open:单帧 ok + 会话元信息
        let reply =
            crate::preview::handle_ctrl_request_reply(state.clone(), open_request(&path, 0, size))
                .await;
        let CtrlReply::Frame(resp) = reply else {
            panic!("open 必单帧");
        };
        assert!(resp.ok, "open 应成: {resp:?}");
        let open = match resp.body {
            Some(ctrl_response::Body::RenditionOpen(o)) => o,
            b => panic!("open 应答体形不符: {b:?}"),
        };
        assert_eq!(open.session_id, 0, "首会话 id = 0(连接内单调分配)");
        assert_eq!(open.segment_ms, 4000);
        assert_eq!(open.mime, "video/iso.segment");

        // seg0..2 逐字节(0 = init)
        for i in 0..3u32 {
            let reply = crate::preview::handle_ctrl_request_reply(
                state.clone(),
                seg_request(open.session_id, i),
            )
            .await;
            let CtrlReply::Stream(out) = reply else {
                panic!("段 {i} 必流式");
            };
            let bytes = drain(out.chunks).await;
            let want = if i == 0 {
                PseudoTranscoder::init_bytes()
            } else {
                PseudoTranscoder::segment_bytes(i)
            };
            assert_eq!(bytes, want, "段 {i} 逐字节一致");
        }

        // 越界 index 3 → 单帧错(T15-M2 止步稳定词 segment_range,与
        // 「段未就绪(等待)」的 segment_pending 可区分,不得共词)
        let reply = crate::preview::handle_ctrl_request_reply(
            state.clone(),
            seg_request(open.session_id, 3),
        )
        .await;
        let (ok, code, msg) = frame_msg(&reply).expect("越界必单帧错");
        assert_eq!((ok, code.as_str()), (false, "segment_range"));
        assert!(msg.contains("越界") && msg.contains("止步"), "越界词面: {msg}");
        assert!(!msg.contains("未就绪"), "止步词不得携等待词(不得共词): {msg}");
        // 会话仍活(越界不是回收面):后续段照常
        let reply =
            crate::preview::handle_ctrl_request_reply(state, seg_request(open.session_id, 1)).await;
        assert!(matches!(reply, CtrlReply::Stream(_)), "越界后会话仍活");
    }

    /// e2e 相③(brief:cancel 后 seg→Err):cancel = 空 ok 单帧(body
    /// None,proto 钉死无独立响应体)→ 段读 NOT_FOUND;cancel 幂等
    /// (未知会话照成);后端 cancel 恰一次。
    #[tokio::test]
    async fn cancel_then_segment_is_error() {
        let state = test_state();
        let (factory, made) = pseudo_factory();
        state.rendition_sessions().install_factory(factory);
        let (_dir, path, size) = make_video();

        let reply =
            crate::preview::handle_ctrl_request_reply(state.clone(), open_request(&path, 0, size))
                .await;
        let CtrlReply::Frame(resp) = reply else {
            panic!("open 必单帧");
        };
        let sid = match resp.body {
            Some(ctrl_response::Body::RenditionOpen(o)) => o.session_id,
            b => panic!("open 应答体形不符: {b:?}"),
        };

        // cancel:空 ok 单帧
        let reply =
            crate::preview::handle_ctrl_request_reply(state.clone(), cancel_request(sid)).await;
        let CtrlReply::Frame(resp) = reply else {
            panic!("cancel 必单帧");
        };
        assert!(resp.ok, "cancel 应成: {resp:?}");
        assert!(resp.body.is_none(), "cancel 无独立响应体(proto 钉死)");

        // cancel 后 seg → NOT_FOUND(会话不存在/已回收词)
        let reply =
            crate::preview::handle_ctrl_request_reply(state.clone(), seg_request(sid, 0)).await;
        let (ok, code, msg) = frame_msg(&reply).expect("cancel 后段读必单帧错");
        assert_eq!((ok, code.as_str()), (false, "NOT_FOUND"));
        assert!(
            msg.contains("已回收") || msg.contains("不存在"),
            "词面: {msg}"
        );

        // 幂等:再 cancel 照成;在册会话清零;后端 cancel 恰一次
        let reply =
            crate::preview::handle_ctrl_request_reply(state.clone(), cancel_request(sid)).await;
        let CtrlReply::Frame(resp) = reply else {
            panic!("cancel 必单帧");
        };
        assert!(resp.ok);
        assert_eq!(state.rendition_sessions().live_sessions(), 0);
        assert_eq!(made.lock().unwrap()[0].cancels(), 1, "后端 cancel 恰一次");
    }

    /// e2e 相④(brief:60s 回收,注入时钟):活动钟注入(假钟拨针,零
    /// 真实等待)——59s 仍活(拉段即续命);+1s = 恰 60s 空闲即回收
    /// (回收即后端 cancel);活动刷新续命(摸过即重计);回收后段读
    /// NotFound;越界 = InvalidInput(与回收可区分)。
    #[tokio::test]
    async fn idle_recycle_with_injected_clock() {
        let clock = Arc::new(Mutex::new(std::time::Instant::now()));
        let now_clock = Arc::clone(&clock);
        let mgr = RenditionSessionMgr::with_clock(
            RENDITION_IDLE_TTL,
            Arc::new(move || *now_clock.lock().unwrap()),
        );
        let (factory, made) = pseudo_factory();
        mgr.install_factory(factory);

        let (sid, _) = mgr
            .open(
                Arc::new(MemSource {
                    data: vec![0u8; 64],
                }),
                &RenditionConfig::DEFAULT,
                "/收件箱/movie.bin",
                0,
            )
            .unwrap();
        assert_eq!(mgr.meta(sid), Some((4000, SEGMENT_MIME)), "会话元信息");

        // 拨 59s:未到界仍活(拉段刷新活动钟)
        *clock.lock().unwrap() += Duration::from_secs(59);
        assert!(mgr.segment(sid, 1).is_ok(), "59s < 60s 仍活");
        // 再拨 60s = 自上次活动恰 60s:回收(拉段续命后重计)
        *clock.lock().unwrap() += Duration::from_secs(60);
        let e = mgr.segment(sid, 0).err().expect("60s 空闲回收后段读必 Err");
        assert_eq!(e.kind(), io::ErrorKind::NotFound, "60s 空闲回收");
        assert_eq!(made.lock().unwrap()[0].cancels(), 1, "回收即后端 cancel");

        // 越界(活会话)= OutOfRange 止步,与回收 NotFound 可区分
        let (sid2, _) = mgr
            .open(
                Arc::new(MemSource {
                    data: vec![0u8; 64],
                }),
                &RenditionConfig::DEFAULT,
                "/收件箱/movie.bin",
                0,
            )
            .unwrap();
        let e = mgr.segment(sid2, 9).err().expect("活会话越界段读必 Err");
        assert!(is_segment_range(&e), "越界止步面(标记类型): {e}");

        // 活动刷新续命:摸过即重计(30s 摸 → 再 30s 仍活;再 60s 即回收)
        *clock.lock().unwrap() += Duration::from_secs(30);
        assert!(mgr.segment(sid2, 0).is_ok(), "30s 摸一次");
        *clock.lock().unwrap() += Duration::from_secs(30);
        assert!(mgr.segment(sid2, 0).is_ok(), "累计 60s 但真空闲 30s:仍活");
        *clock.lock().unwrap() += Duration::from_secs(60);
        assert!(mgr.segment(sid2, 0).is_err(), "真空闲 60s:回收");

        // 显式 cancel 后段读 NotFound
        let (sid3, _) = mgr
            .open(
                Arc::new(MemSource {
                    data: vec![0u8; 64],
                }),
                &RenditionConfig::DEFAULT,
                "/收件箱/movie.bin",
                0,
            )
            .unwrap();
        assert!(mgr.cancel(sid3));
        assert!(mgr.segment(sid3, 0).is_err());
        assert_eq!(
            made.lock()
                .unwrap()
                .iter()
                .map(|t| t.cancels())
                .sum::<u32>(),
            3,
            "三次回收(空闲×2 + 显式×1)各 cancel 一次"
        );
    }

    /// open 无后端 = 稳定词 unsupported 单帧(桌面 v1 生产面;查看端据此
    /// 回退原流+海报)。
    #[tokio::test]
    async fn open_without_backend_is_unsupported_frame() {
        let state = test_state(); // 生产工厂 = platform_backend(恒 None)
        let (_dir, path, size) = make_video();
        let reply =
            crate::preview::handle_ctrl_request_reply(state, open_request(&path, 0, size)).await;
        let (ok, code, msg) = frame_msg(&reply).expect("无后端必单帧错");
        assert_eq!((ok, code.as_str()), (false, "unsupported"));
        assert!(msg.contains("回退"), "词面携回退语义: {msg}");
    }

    /// 声明大小不符(文件已变更)= NOT_FOUND 单帧(对齐 read_range 口径)。
    #[tokio::test]
    async fn open_size_mismatch_rejected() {
        let state = test_state();
        state
            .rendition_sessions()
            .install_factory(pseudo_factory().0);
        let (_dir, path, _size) = make_video();
        let reply =
            crate::preview::handle_ctrl_request_reply(state, open_request(&path, 0, 999)).await;
        assert_eq!(
            frame_code(&reply),
            Some((false, "NOT_FOUND".into())),
            "陈旧声明 NOT_FOUND"
        );
    }

    /// open 臂让出执行器(评审 Important #1 钉面):慢工厂(装配面睡
    /// 200ms,T13/T14 App 桥启动面预演)——open 应答照常到达,且**应答
    /// 在途窗内**探针即观察到工厂进场。判别构造:单线程测试运行时,若
    /// open 内联阻塞执行器,探针只能在应答之后才被调度(观察时旗已落)
    /// = false;阻塞池收口(正形)则探针亚毫秒级进场 = true。open/
    /// segment/cancel 三臂同纪律:后端调用面全在阻塞池,异步 worker 不
    /// 被占死。
    #[tokio::test]
    async fn open_arm_yields_executor_during_slow_factory() {
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering as AtomicOrdering};
        let state = test_state();
        let entered = Arc::new(AtomicBool::new(false));
        let entered_f = Arc::clone(&entered);
        let factory: BackendFactory = Arc::new(move |_src, _path, _start_at| {
            entered_f.store(true, AtomicOrdering::SeqCst);
            std::thread::sleep(Duration::from_millis(200));
            Some(Arc::new(PseudoTranscoder {
                cancels: AtomicU32::new(0),
                duration_ms: std::sync::atomic::AtomicU64::new(0),
            }) as Arc<dyn Transcoder>)
        });
        state.rendition_sessions().install_factory(factory);
        let (_dir, path, size) = make_video();

        // 应答在途旗(reply 产生后置位;探针据此判「观察时是否在途」)
        let reply_done = Arc::new(AtomicBool::new(false));
        let (verdict_tx, verdict_rx) = tokio::sync::oneshot::channel::<bool>();
        let entered_p = Arc::clone(&entered);
        let reply_done_p = Arc::clone(&reply_done);
        tokio::spawn(async move {
            while !entered_p.load(AtomicOrdering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            let _ = verdict_tx.send(!reply_done_p.load(AtomicOrdering::SeqCst));
        });

        let reply_task = tokio::spawn(async move {
            crate::preview::handle_ctrl_request_reply(state, open_request(&path, 0, size)).await
        });
        let reply = reply_task.await.expect("open 任务不崩");
        reply_done.store(true, AtomicOrdering::SeqCst);
        let CtrlReply::Frame(resp) = reply else {
            panic!("open 必单帧");
        };
        assert!(resp.ok, "慢工厂 open 仍应成: {resp:?}");
        let observed_in_flight = verdict_rx.await.expect("探针必达(工厂进场已证)");
        assert!(
            observed_in_flight,
            "执行器让出面:探针须在应答在途窗内观察到工厂进场(内联阻塞 = false)"
        );
    }

    /// 守卫(分发缝):集外路径 rendition open = INVALID_PATH 单帧拒。
    #[tokio::test]
    async fn rendition_open_outside_roots_rejected() {
        let (state, home) = test_state_with_home();
        std::fs::create_dir_all(home.path().join("docs")).unwrap();
        std::fs::create_dir_all(home.path().join("secret")).unwrap();
        std::fs::write(home.path().join("secret").join("movie.bin"), b"s").unwrap();
        state.set_roots(vec![RootSpec {
            name: "docs".into(),
            path: home.path().join("docs").to_string_lossy().into_owned(),
            source: RootSource::Fs,
            visible: true,
        }]);
        let reply = crate::preview::handle_ctrl_request_reply(
            state,
            open_request(
                home.path()
                    .join("secret")
                    .join("movie.bin")
                    .to_string_lossy()
                    .as_ref(),
                0,
                1,
            ),
        )
        .await;
        assert_eq!(
            frame_code(&reply),
            Some((false, "INVALID_PATH".into())),
            "集外 rendition open INVALID_PATH"
        );
    }

    /// 桥 vpath 路由(直通臂):bridge 根注册(App 缺席 = 零订阅快失败)
    /// → source_offline(导出不可达),而非 fs 面错误——open 的 vpath 臂
    /// 真实路由到桥导轨(对齐 read_range 桥测面)。
    #[tokio::test]
    async fn rendition_open_bridge_vpath_routes_bridge_rail() {
        let (state, home) = test_state_with_home();
        let root = home.path().join("桥根");
        std::fs::create_dir_all(&root).unwrap();
        state.set_roots(vec![RootSpec {
            name: "桥".into(),
            path: root.to_string_lossy().into_owned(),
            source: RootSource::Bridge,
            visible: true,
        }]);
        state
            .rendition_sessions()
            .install_factory(pseudo_factory().0);
        let vpath = format!("{}{}movie.bin", root.display(), std::path::MAIN_SEPARATOR);
        let reply =
            crate::preview::handle_ctrl_request_reply(state, open_request(&vpath, 0, 0)).await;
        let (ok, code, msg) = frame_msg(&reply).expect("桥 vpath 落桥臂 = 单帧错");
        assert_eq!((ok, code.as_str()), (false, "source_offline"));
        assert!(
            msg.contains("服务") || msg.contains("导出"),
            "桥导轨词面(非 fs 臂「源打开失败」): {msg}"
        );
    }

    /// 照片 vpath(photo_serve 关):落既有守卫按集外拒(INVALID_PATH,
    /// 行为与泛化前逐字节一致;unix 门:photo_vpath 词法规范化按平台绝对
    /// 形态收口,preview.rs 照片臂同门)。
    #[cfg(unix)]
    #[tokio::test]
    async fn rendition_open_photo_vpath_photo_off_rejected_by_guard() {
        let (state, _home) = test_state_with_home();
        std::fs::create_dir_all(_home.path().join("docs")).unwrap();
        state.set_roots(vec![RootSpec {
            name: "docs".into(),
            path: _home.path().join("docs").to_string_lossy().into_owned(),
            source: RootSource::Fs,
            visible: true,
        }]);
        let reply = crate::preview::handle_ctrl_request_reply(
            state,
            open_request("/照片图库/相册/movie.mp4", 0, 1),
        )
        .await;
        assert_eq!(
            frame_code(&reply),
            Some((false, "INVALID_PATH".into())),
            "photo_serve 关 = 既有集外拒(行为不变)"
        );
    }

    /// 照片 vpath(photo_serve 开):守卫照片臂放行(T9/T10 口径对齐)→
    /// 桥先行(无订阅快败)→ 照片单查快败 = source_offline 携照片词。
    #[cfg(unix)]
    #[tokio::test]
    async fn rendition_open_photo_vpath_routes_photo_rail() {
        let (events, _) = tokio::sync::broadcast::channel(16);
        let home = tempfile::tempdir().unwrap();
        {
            let session = crate::session::Session {
                server: "http://127.0.0.1:9".into(),
                jwt: "test".into(),
                refresh: "test".into(),
                device_id: "selfdev".into(),
                ca: None,
            };
            session.save(home.path()).unwrap();
        }
        #[cfg(windows)]
        let update = std::sync::Arc::new(crate::update::UpdateService::new(
            crate::update::UpdateConfig::default(),
            events.clone(),
            tokio::sync::watch::channel(false).0,
        ));
        #[cfg(not(windows))]
        let update = ();
        let state = Arc::new(
            DaemonState::load(
                home.path(),
                home.path().join("inbox"),
                None,
                inbox_dest::staging_dir(home.path()),
                false,
                true, // photo_serve 开(照片桥随 state 常驻)
                Default::default(),
                Default::default(),
                crate::ctrl::ctrl_default_tuning(),
                events,
                update,
            )
            .unwrap(),
        );
        state
            .rendition_sessions()
            .install_factory(pseudo_factory().0);
        let reply = crate::preview::handle_ctrl_request_reply(
            state,
            open_request("/照片图库/相册/movie.mp4", 0, 0),
        )
        .await;
        let (ok, code, msg) = frame_msg(&reply).expect("照片臂单帧错");
        assert_eq!((ok, code.as_str()), (false, "source_offline"));
        assert!(msg.contains("照片"), "照片词(非 fs 臂): {msg}");
    }

    /// r29:open 应答携 duration_ms(伪管线报 26s)→ playlist **VOD 全列**:
    /// 26s/4s = 7 段一次列出 + ENDLIST(时长自第一帧即全长;未拉段照列,
    /// 段路由有界等待/远跳重开兜)。
    #[tokio::test]
    async fn viewer_playlist_vod_full_listing_when_duration_known() {
        let source = test_state();
        let made: Arc<Mutex<Vec<Arc<PseudoTranscoder>>>> = Arc::new(Mutex::new(Vec::new()));
        let made_c = Arc::clone(&made);
        let factory: BackendFactory = Arc::new(move |_s, _p, _st| {
            let t = Arc::new(PseudoTranscoder {
                cancels: std::sync::atomic::AtomicU32::new(0),
                duration_ms: std::sync::atomic::AtomicU64::new(26_000),
            });
            made_c.lock().unwrap().push(Arc::clone(&t));
            Some(t as Arc<dyn Transcoder>)
        });
        source.rendition_sessions().install_factory(factory);
        let (_dir, path, size) = make_video();

        let fetch = RenditionFetch::local(&source);
        let v = ViewerRendition::attach(fetch, &path, 0, size).await.unwrap();

        // 零拉取(段 0 未落盘)playlist 即全长列出 + ENDLIST
        let pl = v.playlist_m3u8(T_SEG_PREFIX, T_TOKEN);
        for i in 1..=7u32 {
            assert!(
                pl.contains(&format!("/{i}.m4s?t={T_TOKEN}")),
                "段 {i} 全列(26s/4s=7): {pl}"
            );
        }
        assert!(!pl.contains("/8.m4s"), "越总时长段不列: {pl}");
        assert!(pl.contains("#EXT-X-ENDLIST"), "VOD 形即封 ENDLIST: {pl}");
    }

    /// 查看端装配全链路:attach(本地直驱面)→ pull_segments(段 0..2
    /// 入 T5 缓存,逐字节核对缓存件)→ playlist 形状(VERSION:7/EVENT/
    /// TARGETDURATION/MAP/EXTINF,无 ENDLIST)→ 未拉段无路径 → detach
    /// (源端会话回收,后续段读 NOT_FOUND)。
    #[tokio::test]
    async fn viewer_assembly_pull_playlist_and_detach() {
        let source = test_state();
        let (factory, made) = pseudo_factory();
        source.rendition_sessions().install_factory(factory);
        let (_dir, path, size) = make_video();

        let fetch = RenditionFetch::local(&source);
        let v = ViewerRendition::attach(fetch, &path, 0, size)
            .await
            .unwrap();

        let cache = source.preview_cache();
        // 测试态的 home TempDir 随构造收尾(目录已被删):缓存目录随手
        // 重建(cache.rs 目录名锚点;空目录 = 空账起步,记账/对账面照常)。
        std::fs::create_dir_all(source.home().join("preview-tmp")).unwrap();
        let prefix = "deadbeefdeadbeefdeadbeefdeadbeef"; // 32 hex(handle 形)
        let pulled = v.pull_segments(&cache, prefix).await.unwrap();
        assert_eq!(pulled, 3, "init + 2 内容段全拉(至越界止步)");
        for i in 0..3u32 {
            let p = v.segment_path(&cache, prefix, i).expect("已拉段有路径");
            let want = if i == 0 {
                PseudoTranscoder::init_bytes()
            } else {
                PseudoTranscoder::segment_bytes(i)
            };
            let mut got = Vec::new();
            std::fs::File::open(&p)
                .unwrap()
                .read_to_end(&mut got)
                .unwrap();
            assert_eq!(got, want, "段 {i} 缓存件逐字节一致");
        }
        assert!(v.segment_path(&cache, prefix, 3).is_none(), "未拉段无路径");
        assert_eq!(v.mime(), "video/iso.segment");

        // playlist 形状(brief:#EXT-X-VERSION:7 / fMP4 / #EXT-X-MAP;段 URI
        // 绝对+携 ?t= 令牌——T21 fix round 1 Critical #1:相对 URI 经 hls.js
        // RFC 3986 解析丢查询串即段请求 401)
        let pl = v.playlist_m3u8(T_SEG_PREFIX, T_TOKEN);
        assert!(pl.starts_with("#EXTM3U\n"), "M3U8 头: {pl}");
        assert!(pl.contains("#EXT-X-VERSION:7"), "版本 7: {pl}");
        assert!(pl.contains("#EXT-X-PLAYLIST-TYPE:EVENT"), "动态 EVENT 形");
        assert!(pl.contains("#EXT-X-TARGETDURATION:4"), "4000ms → 4s");
        assert!(
            pl.contains(&format!("#EXT-X-MAP:URI=\"{T_SEG_PREFIX}/0.m4s?t={T_TOKEN}\"")),
            "fMP4 MAP 面(绝对+令牌): {pl}"
        );
        assert!(
            pl.contains(&format!("#EXTINF:4.000,\n{T_SEG_PREFIX}/1.m4s?t={T_TOKEN}\n")),
            "内容段 1(绝对+令牌): {pl}"
        );
        assert!(pl.contains("seg/2.m4s?t="), "内容段 2(段行同携令牌)");
        // field 修:伪转码器段尽(越界止步)= 源端完毕 → playlist 封
        // ENDLIST(时长锁定全长 + 播放端结束态)
        assert!(pl.contains("#EXT-X-ENDLIST"), "段尽封 ENDLIST: {pl}");

        // detach:源端会话回收,后续段读 NOT_FOUND
        let sid = v.session_id();
        v.detach().await;
        assert_eq!(
            source.rendition_sessions().live_sessions(),
            0,
            "detach 即回收"
        );
        let reply =
            crate::preview::handle_ctrl_request_reply(Arc::clone(&source), seg_request(sid, 0))
                .await;
        assert_eq!(
            frame_code(&reply),
            Some((false, "NOT_FOUND".into())),
            "detach 后段读 NOT_FOUND"
        );
        assert_eq!(made.lock().unwrap()[0].cancels(), 1, "detach 即后端 cancel");
    }

    /// 查看端装配:源端无后端 = AttachError::Unsupported(回退原流+海报
    /// 的触发面);源缺失 = Failed(source_offline)。
    #[tokio::test]
    async fn viewer_attach_unsupported_and_missing_source() {
        let source = test_state(); // 生产工厂 None
        let (_dir, path, size) = make_video();
        let fetch = RenditionFetch::local(&source);
        let err = ViewerRendition::attach(fetch, &path, 0, size)
            .await
            .err()
            .expect("无后端 attach 必 Err");
        assert!(
            matches!(err, AttachError::Unsupported),
            "无后端 = Unsupported: {err:?}"
        );
        let fetch = RenditionFetch::local(&source);
        let err = ViewerRendition::attach(fetch, "/不存在的/movie.bin", 0, 1)
            .await
            .err()
            .expect("源缺失 attach 必 Err");
        match err {
            AttachError::Failed(code, msg) => {
                assert_eq!(code, "source_offline");
                assert!(!msg.is_empty(), "文案在场(T15 日志/降级提示面)");
            }
            e => panic!("源缺失 = Failed: {e:?}"),
        }
    }

    /// 查看端装配:首段(init)拉不到 = pull Err(Review#1:503 面原料);
    /// 会话被背后回收后 pull 即此面。
    #[tokio::test]
    async fn viewer_pull_first_segment_fail_is_err() {
        let source = test_state();
        let (factory, _made) = pseudo_factory();
        source.rendition_sessions().install_factory(factory);
        let (_dir, path, size) = make_video();
        let fetch = RenditionFetch::local(&source);
        let v = ViewerRendition::attach(fetch, &path, 0, size)
            .await
            .unwrap();
        source.rendition_sessions().cancel(v.session_id());
        let cache = source.preview_cache();
        let err = v
            .pull_segments(&cache, "deadbeefdeadbeefdeadbeefdeadbeef")
            .await
            .unwrap_err();
        assert_eq!(err.0, "NOT_FOUND", "init 拉不到(会话已回收)= Err: {err:?}");
    }

    /// 常量锚(T15):跳段有界等待窗 10s(brief 钉死)/探测间隔 500ms/
    /// 前缀推进窗 8 段;T15b:远跳重开触发 = 窗过半(×2 == 等待窗)/
    /// 重试稳定词与 Retry-After 秒值(T21 词面契约)。
    #[test]
    fn rendition_seg_constants() {
        assert_eq!(RENDITION_SEG_WAIT, Duration::from_secs(10), "等待窗 10s");
        assert_eq!(RENDITION_SEG_POLL, Duration::from_millis(500), "探测间隔");
        assert_eq!(RENDITION_PULL_WINDOW, 16, "单次前缀推进窗(field 修:8→16,段产快于播放时更快追全长)");
        assert_eq!(
            RENDITION_SEG_REOPEN_AT * 2,
            RENDITION_SEG_WAIT,
            "远跳重开触发 = 窗过半(总界不变)"
        );
        assert_eq!(SEG_RETRY_WORD, "seg_retry", "重试稳定词(T21 词面契约)");
        assert_eq!(SEG_RETRY_AFTER_SECS, 2, "Retry-After 秒值");
    }

    /// T12-M2 稳定词钉面:段未就绪(等待)与段越界(止步)**两可区分词,
    /// 不得共词**——pending 词面携「未就绪」无「越界」;range 词面携
    /// 「越界」无「未就绪」;稳定码互异(segment_pending/segment_range)。
    #[test]
    fn segment_err_words_distinguishable() {
        let pending = segment_err_response(&err_segment_pending(7));
        let range = segment_err_response(&err_segment_range(7));
        let (pcode, pmsg) = frame_word(&pending);
        let (rcode, rmsg) = frame_word(&range);
        assert_eq!(pcode, "segment_pending", "等待面稳定码: {pmsg}");
        assert_eq!(rcode, "segment_range", "止步面稳定码: {rmsg}");
        assert!(pmsg.contains("未就绪"), "等待词面: {pmsg}");
        assert!(!pmsg.contains("越界"), "等待词不得携止步词: {pmsg}");
        assert!(rmsg.contains("越界") && rmsg.contains("止步"), "止步词面: {rmsg}");
        assert!(!rmsg.contains("未就绪"), "止步词不得携等待词(不得共词): {rmsg}");
    }

    /// 稳定词投影(单帧 Error 体 (code, message);测试面)。
    fn frame_word(resp: &CtrlResponse) -> (String, String) {
        match resp.body.as_ref() {
            Some(ctrl_response::Body::Error(e)) => (e.code.clone(), e.message.clone()),
            b => panic!("应答体形不符: {b:?}"),
        }
    }

    /// T12-M5 钉面:拉段失败面(source_offline)分类先于重试/止步逻辑
    /// ——init 后中途读败 = pull **Err**(勿吞为段尽;旧形 break 即红);
    /// 恒读败 = init 臂即 Err。对照:pending/range 止步面 = Ok(前缀保留)。
    #[tokio::test]
    async fn viewer_pull_offline_not_swallowed_as_end() {
        let source = test_state();
        let (factory, _made) = test_factory(FakeTranscoder::content_failing);
        source.rendition_sessions().install_factory(factory);
        let (_dir, path, size) = make_video();
        let v = ViewerRendition::attach(RenditionFetch::local(&source), &path, 0, size)
            .await
            .unwrap();
        let cache = source.preview_cache();
        std::fs::create_dir_all(source.home().join("preview-tmp")).unwrap();
        let err = v
            .pull_segments(&cache, "deadbeefdeadbeefdeadbeefdeadbeef")
            .await
            .unwrap_err();
        assert_eq!(err.0, "source_offline", "中途失败勿吞为段尽: {err:?}");

        // 对照臂:pending/range 止步 = Ok(前缀照推)
        let source2 = test_state();
        let (f2, made2) = test_factory(FakeTranscoder::pending);
        source2.rendition_sessions().install_factory(f2);
        let (_dir2, path2, size2) = make_video();
        let v2 = ViewerRendition::attach(RenditionFetch::local(&source2), &path2, 0, size2)
            .await
            .unwrap();
        let cache2 = source2.preview_cache();
        std::fs::create_dir_all(source2.home().join("preview-tmp")).unwrap();
        let pulled = v2
            .pull_segments(&cache2, "deadbeefdeadbeefdeadbeefdeadbeef")
            .await
            .unwrap();
        assert_eq!(pulled, 1, "init 后恒未就绪 = 前缀止步于 1(init)");
        assert_eq!(made2.lock().unwrap()[0].calls(), [0, 1], "init 产出 + 探测下一面即止");
    }

    /// 跳段(T15 主面):前缀外段直取——**乱序直达源端**(调用序记录面
    /// 即「源端收到跳段请求」的桥面证词),到手落缓存、前缀不越推;随后
    /// 顺序推进对已拉段缓存短路(零重拉),playlist 段序无洞补齐。
    #[tokio::test]
    async fn viewer_skip_direct_fetch_then_prefix_backfills() {
        let source = test_state();
        let (factory, made) = test_factory(FakeTranscoder::standard);
        source.rendition_sessions().install_factory(factory);
        let (_dir, path, size) = make_video();
        let v = ViewerRendition::attach(RenditionFetch::local(&source), &path, 0, size)
            .await
            .unwrap();
        let cache = source.preview_cache();
        std::fs::create_dir_all(source.home().join("preview-tmp")).unwrap();
        let prefix = "deadbeefdeadbeefdeadbeefdeadbeef";

        // 未拉任何段即请求 seg/2(拖动面):直取目标段
        let bytes = v.ensure_segment(&cache, prefix, 2).await.unwrap();
        assert_eq!(bytes, PseudoTranscoder::segment_bytes(2), "跳段字节一致");
        assert_eq!(made.lock().unwrap()[0].calls(), [2], "源端收到乱序跳段请求(直取 2)");
        assert_eq!(v.pulled_now(), 0, "跳段不越推前缀(playlist 段序无洞纪律)");
        assert!(
            !v.playlist_m3u8(T_SEG_PREFIX, T_TOKEN).contains("seg/1"),
            "前缀未至不列(未拉段不列): {}",
            v.playlist_m3u8(T_SEG_PREFIX, T_TOKEN)
        );

        // 顺序推进:2 已在盘短路,只拉 0/1;再探下一面(index 3 越界止步)
        let pulled = v.pull_segments(&cache, prefix).await.unwrap();
        assert_eq!(pulled, 3);
        assert_eq!(
            made.lock().unwrap()[0].calls(),
            [2, 0, 1, 3],
            "顺序推进对跳段已拉段零重拉(M3 短路面);段尽探测一发即止"
        );
        let pl = v.playlist_m3u8(T_SEG_PREFIX, T_TOKEN);
        assert!(pl.contains("seg/1.m4s?t=") && pl.contains("seg/2.m4s?t="), "前缀补齐: {pl}");
        assert!(!pl.contains("seg/3"), "未产段不列: {pl}");

        // T12-M3:LRU 驱逐面(删盘上缓存件)→ 段路由消费时过存在性,回源重拉
        let p1 = v.segment_path(&cache, prefix, 1).expect("段 1 在盘");
        std::fs::remove_file(&p1).unwrap();
        assert!(v.segment_path(&cache, prefix, 1).is_none(), "盘上缺席 = None(存在性面)");
        let bytes = v.ensure_segment(&cache, prefix, 1).await.unwrap();
        assert_eq!(bytes, PseudoTranscoder::segment_bytes(1), "驱逐后回源重拉字节一致");
        assert_eq!(
            made.lock().unwrap()[0].calls(),
            [2, 0, 1, 3, 1],
            "重拉即一次源端请求(段尽探测后追加一发段 1)"
        );
    }

    /// 跳段有界等待(brief:「有界等待 10s→503」):恒未就绪源——窗烧尽
    /// = Timeout 面(503 原料);tokio 暂停钟注入(拨钟即烧窗,零真实
    /// 等待),tokio Instant 断言整窗烧满。T15b:窗过半恰一次远跳重开
    /// (start_at = 目标段×段长),总界不因重开放宽。
    #[tokio::test(start_paused = true)]
    async fn viewer_seg_wait_burns_budget_then_timeout() {
        let source = test_state();
        let (factory, made, starts) = test_factory_start_at(|_| FakeTranscoder::pending());
        source.rendition_sessions().install_factory(factory);
        let (_dir, path, size) = make_video();
        let v = ViewerRendition::attach(RenditionFetch::local(&source), &path, 0, size)
            .await
            .unwrap();
        let cache = source.preview_cache();
        std::fs::create_dir_all(source.home().join("preview-tmp")).unwrap();
        let t0 = tokio::time::Instant::now();
        let err = v
            .ensure_segment(&cache, "deadbeefdeadbeefdeadbeefdeadbeef", 1)
            .await
            .unwrap_err();
        assert!(matches!(err, SegFail::Timeout), "窗烧尽 = Timeout: {err:?}");
        assert!(
            t0.elapsed() >= RENDITION_SEG_WAIT,
            "整窗烧满(注入钟面): {:?}",
            t0.elapsed()
        );
        assert!(
            t0.elapsed() < RENDITION_SEG_WAIT + RENDITION_SEG_POLL * 2,
            "窗即止不悬挂: {:?}",
            t0.elapsed()
        );
        assert_eq!(
            starts.lock().unwrap().as_slice(),
            [0, 4_000],
            "窗过半恰一次远跳重开(start_at = 目标段×段长)"
        );
        assert_eq!(made.lock().unwrap()[0].cancels(), 1, "旧会话 cancel 恰一次");
        assert!(
            made.lock().unwrap()[0].calls().len() >= 2,
            "窗内多轮探测"
        );
    }

    /// T15b 远跳重开主面(start_paused 注入钟):start_at 敏感伪转码器
    /// (从头会话恒段未就绪 = v1 死等面;重开会话即产 SEEKED 段)——远段
    /// 请求窗过半自动 cancel+重开(start_at = 目标段×段长)+余窗续达;
    /// playlist 按新会话重建(自锚列段,段索引空间沿用,**init 段随重开
    /// 作废覆写**——重开后的 init 字节 = 新会话产出,MAP 面不携旧会话
    /// moov);重开洞域(锚前段号)= 止步;已拉段缓存命中零新会话。
    #[tokio::test(start_paused = true)]
    async fn viewer_far_seek_reopens_with_start_at_and_rebuilds_playlist() {
        let source = test_state();
        let (factory, made, starts) = test_factory_start_at(FakeTranscoder::gated);
        source.rendition_sessions().install_factory(factory);
        let (_dir, path, size) = make_video();
        let v = ViewerRendition::attach(RenditionFetch::local(&source), &path, 0, size)
            .await
            .unwrap();
        let cache = source.preview_cache();
        std::fs::create_dir_all(source.home().join("preview-tmp")).unwrap();
        let prefix = "deadbeefdeadbeefdeadbeefdeadbeef";

        // 远跳:目标段 2(从头会话恒未就绪 → 窗过半重开 → 余窗内 SEEKED 到手)
        let bytes = v.ensure_segment(&cache, prefix, 2).await.unwrap();
        assert_eq!(
            bytes,
            FakeTranscoder::seeked_bytes(1),
            "重开会话段 1 字节(= 查看端段 2,索引空间沿用)"
        );
        assert_eq!(
            starts.lock().unwrap().as_slice(),
            [0, 8_000],
            "源端收到重开请求且 start_at = 目标段×段长"
        );
        assert_eq!(made.lock().unwrap()[0].cancels(), 1, "旧会话 cancel 恰一次(cancel 先行)");

        // playlist 刷新:init 前奏(重开作废旧 init → 拉新会话 init 覆写)
        // + 锚域推进(段 2 已盘短路 + 顺序拉段 3 → 越界止步)
        let pulled = v.pull_segments(&cache, prefix).await.unwrap();
        assert_eq!(pulled, 3, "init(新会话)+ 锚域段 2 + 顺序段 3");
        // init 覆写钉面:盘上 seg/0 = 新会话 init(SEEKED 管线起步产出,
        // 非旧会话 moov——旧形缓存件属旧会话代次即红)
        let p0 = v.segment_path(&cache, prefix, 0).expect("init 在盘");
        let mut got0 = Vec::new();
        std::fs::File::open(&p0)
            .unwrap()
            .read_to_end(&mut got0)
            .unwrap();
        assert_eq!(got0, FakeTranscoder::seeked_bytes(0), "init 段 = 重开会话产出(覆写旧 moov)");
        assert_eq!(v.pulled_now(), 3);
        let pl = v.playlist_m3u8(T_SEG_PREFIX, T_TOKEN);
        assert!(
            pl.contains(&format!("#EXTINF:4.000,\n{T_SEG_PREFIX}/2.m4s?t={T_TOKEN}\n"))
                && pl.contains("seg/3.m4s?t="),
            "playlist 按新会话重建(自重开锚列段,段索引空间沿用): {pl}"
        );
        assert!(!pl.contains("seg/1"), "旧会话段不列(重建面): {pl}");

        // 重开洞域:锚(2)之前的段 1 = 止步(旧会话已 cancel,新会话不产)
        let err = v.ensure_segment(&cache, prefix, 1).await.unwrap_err();
        assert!(matches!(err, SegFail::Range), "洞域止步: {err:?}");
        assert_eq!(starts.lock().unwrap().as_slice(), [0, 8_000], "洞域止步零新会话");

        // 缓存命中:段 2 再取零新会话(字节一致)
        let bytes = v.ensure_segment(&cache, prefix, 2).await.unwrap();
        assert_eq!(bytes, FakeTranscoder::seeked_bytes(1), "缓存命中字节一致");
        assert_eq!(starts.lock().unwrap().as_slice(), [0, 8_000], "命中零新会话");
    }

    /// T15b 每等待至多重开一枚(本地旗):恒未就绪源,整窗烧尽 = Timeout,
    /// 源端恰见一次重开(勿逐 poll 连环 cancel+open);总界不放宽。
    #[tokio::test(start_paused = true)]
    async fn viewer_far_seek_reopens_at_most_once_per_wait() {
        let source = test_state();
        let (factory, made, starts) = test_factory_start_at(|_| FakeTranscoder::pending());
        source.rendition_sessions().install_factory(factory);
        let (_dir, path, size) = make_video();
        let v = ViewerRendition::attach(RenditionFetch::local(&source), &path, 0, size)
            .await
            .unwrap();
        let cache = source.preview_cache();
        std::fs::create_dir_all(source.home().join("preview-tmp")).unwrap();
        let t0 = tokio::time::Instant::now();
        let err = v
            .ensure_segment(&cache, "deadbeefdeadbeefdeadbeefdeadbeef", 5)
            .await
            .unwrap_err();
        assert!(matches!(err, SegFail::Timeout), "重开后续等仍尽 = Timeout: {err:?}");
        assert!(
            t0.elapsed() >= RENDITION_SEG_WAIT,
            "总界 10s 不变(重开只换道不放宽): {:?}",
            t0.elapsed()
        );
        assert!(
            t0.elapsed() < RENDITION_SEG_WAIT + RENDITION_SEG_POLL * 2,
            "窗即止不悬挂: {:?}",
            t0.elapsed()
        );
        assert_eq!(
            starts.lock().unwrap().as_slice(),
            [0, 20_000],
            "恰一次重开(start_at = 5×4000),非逐 poll 连环"
        );
        assert_eq!(made.lock().unwrap()[0].cancels(), 1, "旧会话 cancel 恰一次");
    }

    /// T15b 并发同目标判重(代次):两请求同窗等同一远段,窗过半双双触发
    /// ——串行门 + 代次判重收敛为一枚新会话(勿两会话互踩双开)。
    #[tokio::test(start_paused = true)]
    async fn viewer_far_seek_concurrent_reopen_dedupes_by_generation() {
        let source = test_state();
        let (factory, _made, starts) = test_factory_start_at(|_| FakeTranscoder::pending());
        source.rendition_sessions().install_factory(factory);
        let (_dir, path, size) = make_video();
        let v = std::sync::Arc::new(
            ViewerRendition::attach(RenditionFetch::local(&source), &path, 0, size)
                .await
                .unwrap(),
        );
        let cache = source.preview_cache();
        std::fs::create_dir_all(source.home().join("preview-tmp")).unwrap();
        let v2 = std::sync::Arc::clone(&v);
        let v3 = std::sync::Arc::clone(&v);
        let (a, b) = tokio::join!(
            async { v2.ensure_segment(&cache, "deadbeefdeadbeefdeadbeefdeadbeef", 5).await },
            async { v3.ensure_segment(&cache, "deadbeefdeadbeefdeadbeefdeadbeef", 5).await },
        );
        assert!(
            matches!(a, Err(SegFail::Timeout)) && matches!(b, Err(SegFail::Timeout)),
            "双请求各按窗尽收口"
        );
        assert_eq!(
            starts.lock().unwrap().as_slice(),
            [0, 20_000],
            "并发双触发收敛一枚新会话(代次判重)"
        );
    }

    /// T15b 源端透传:rendition_open 的 start_at_ms 原样达工厂(App 桥
    /// 后端以此为 bridge.open 起点,T13/T14 桥事件载荷同字段);**同会话
    /// 重复 open 不同 start_at = 新会话**(id 单调不复用,App 侧即重开
    /// 管线);两会话并存(旧会话回收归查看端 detach/源端 60s 空闲兜底)。
    #[tokio::test]
    async fn rendition_open_passes_start_at_and_reopen_is_new_session() {
        let state = test_state();
        let (factory, _made, starts) =
            test_factory_start_at(|_s| PseudoTranscoder {
                cancels: std::sync::atomic::AtomicU32::new(0),
                duration_ms: std::sync::atomic::AtomicU64::new(0),
            });
        state.rendition_sessions().install_factory(factory);
        let (_dir, path, size) = make_video();
        let reply = crate::preview::handle_ctrl_request_reply(
            state.clone(),
            open_request_start_at(&path, 0, size, 12_345),
        )
        .await;
        let CtrlReply::Frame(resp) = reply else {
            panic!("open 必单帧");
        };
        assert!(resp.ok, "带 start_at open 应成: {resp:?}");
        let sid1 = match resp.body {
            Some(ctrl_response::Body::RenditionOpen(o)) => o.session_id,
            b => panic!("open 应答体形不符: {b:?}"),
        };
        assert_eq!(
            starts.lock().unwrap().as_slice(),
            [12_345],
            "start_at_ms 原样透传工厂(桥 open 起点原料)"
        );
        let reply = crate::preview::handle_ctrl_request_reply(
            state.clone(),
            open_request_start_at(&path, 0, size, 48_000),
        )
        .await;
        let CtrlReply::Frame(resp) = reply else {
            panic!("open 必单帧");
        };
        let sid2 = match resp.body {
            Some(ctrl_response::Body::RenditionOpen(o)) => o.session_id,
            b => panic!("open 应答体形不符: {b:?}"),
        };
        assert_ne!(sid1, sid2, "重复 open 不同 start_at = 新会话(App 侧重开管线)");
        assert_eq!(starts.lock().unwrap().as_slice(), [12_345, 48_000]);
        assert_eq!(
            state.rendition_sessions().live_sessions(),
            2,
            "两会话并存(旧未显式 cancel 面)"
        );
    }

    /// 跳段等待窗内成功面:段 1 先 pending 数轮再产出(生产追上面)——
    /// 窗内到手 = Ok,全程 < 窗。
    #[tokio::test(start_paused = true)]
    async fn viewer_seg_wait_succeeds_within_budget() {
        let source = test_state();
        let (factory, made) = test_factory(|| FakeTranscoder::deferred(3));
        source.rendition_sessions().install_factory(factory);
        let (_dir, path, size) = make_video();
        let v = ViewerRendition::attach(RenditionFetch::local(&source), &path, 0, size)
            .await
            .unwrap();
        let cache = source.preview_cache();
        std::fs::create_dir_all(source.home().join("preview-tmp")).unwrap();
        let t0 = tokio::time::Instant::now();
        let bytes = v
            .ensure_segment(&cache, "deadbeefdeadbeefdeadbeefdeadbeef", 1)
            .await
            .unwrap();
        assert_eq!(bytes, PseudoTranscoder::segment_bytes(1), "窗内产出即到手");
        assert!(
            t0.elapsed() < RENDITION_SEG_WAIT,
            "窗内成功: {:?}",
            t0.elapsed()
        );
        assert_eq!(
            made.lock().unwrap()[0].calls(),
            [1, 1, 1, 1],
            "3 轮 pending + 1 轮产出(poll 面真实)"
        );
    }

    /// 空闲回收 ticker(spawn_sweeper):真实钟下 pace = ttl/4 的常驻轮;
    /// abort 即协作收尾(await 返回 cancelled = 任务终局,不泄漏)。回收
    /// 语义归注入钟用例,此处只钉任务生命周期。
    #[tokio::test]
    async fn sweeper_task_lifecycle() {
        let mgr = Arc::new(RenditionSessionMgr::with_clock(
            Duration::from_secs(60),
            Arc::new(std::time::Instant::now),
        ));
        let h = mgr.spawn_sweeper();
        assert!(!h.is_finished());
        h.abort();
        let _ = h.await; // cancelled 收尾(JoinHandle await 即消费)
    }

    /// target 校验(T12-M4):未知形态(非 HLS=0)拒答稳定词
    /// `unsupported_target`,勿静默按 HLS——新端新形态投旧端的定向降级
    /// 面(与「本端无后端」的 unsupported 可区分)。
    #[tokio::test]
    async fn rendition_open_unknown_target_rejected() {
        let state = test_state();
        let (_dir, path, size) = make_video();
        let req = CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: Some(Req::RenditionOpen(RenditionOpenRequest {
                path: path.to_string(),
                mtime: 0,
                size: size as i64,
                target: 7, // 未知形态(本版枚举仅 HLS=0)
                start_at_ms: 0,
            })),
        };
        let reply = crate::preview::handle_ctrl_request_reply(state, req).await;
        let (ok, code, msg) = frame_msg(&reply).expect("未知 target 必单帧拒");
        assert_eq!((ok, code.as_str()), (false, "unsupported_target"));
        assert!(msg.contains("7"), "词面携原值: {msg}");
        assert!(msg.contains("HLS"), "词面携本端支持面: {msg}");
    }

    /// Android 桥后端全链路(V4-1 T13;unix 门 + 多线程运行时——工厂的
    /// block_on 由阻塞池承载):伪 App(事件订阅任务:收 fs.rendition_open
    /// → 落 init/内容段真件 → 0x0C 回执 + 段路径回告)直驱 RenditionBridge
    /// → open 全臂(分发缝)拿到会话 → 段读逐字节(读已产段)→ 未产段
    /// InvalidInput「段未就绪」→ 管线失败标记后新段即 Err 勿烧等待 → 回
    /// 告越界路径拒读(fail-closed)→ cancel 广播 + 摘账 + 会话回收。
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bridge_backend_full_flow_open_segment_cancel() {
        let state = test_state();
        let (events, mut rx) = tokio::sync::broadcast::channel(16);
        let mut rx_cancel = events.subscribe(); // cancel 事件观察面(广播各收各的)
        let bridge = Arc::new(crate::jobs::RenditionBridge::with_timeout(
            events,
            Duration::from_secs(5),
        ));
        let serve = tempfile::tempdir().unwrap();
        let serve_dir = serve.path().join("rendition-serve");
        std::fs::create_dir_all(&serve_dir).unwrap();
        state.rendition_sessions().install_factory(
            super::bridge_backend(Arc::clone(&bridge), serve_dir.clone()),
        );
        let (_dir, path, size) = make_video();

        // 伪 App:open 事件 → 落段真件(init=0/内容=1)→ 回执 + 段回告;
        // cancel 事件即退(段文件由真实 App 的 cancel 清理面承担,测试面
        // 由 serve tempdir 收尾)。
        let app_bridge = Arc::clone(&bridge);
        let app_serve = serve_dir.clone();
        tokio::spawn(async move {
            loop {
                let ev = match rx.recv().await {
                    Ok(ev) => ev,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    Err(_) => continue,
                };
                if ev.method != crate::jobs::EV_RENDITION_OPEN {
                    continue;
                }
                let id = ev.params["request_id"].as_u64().unwrap();
                let dir = app_serve.join(id.to_string());
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("0.m4s"), b"PSEUDO-ANDROID-INIT").unwrap();
                std::fs::write(dir.join("1.m4s"), b"PSEUDO-ANDROID-SEG1").unwrap();
                // 回执先行(真 App 同序:受理即回,产段异步回告;线上两序
                // 均合法——deliver 按形路由,序无关)
                assert!(app_bridge.deliver(id, &serde_json::json!({ "ok": true, "request_id": id })));
                for (i, name) in [(0u32, "0.m4s"), (1, "1.m4s")] {
                    assert!(app_bridge.deliver(
                        id,
                        &serde_json::json!({
                            "index": i, "ok": true,
                            "path": dir.join(name).to_string_lossy(), "request_id": id,
                        })
                    ));
                }
            }
        });

        // open 全臂:单帧 ok(session/segment_ms/mime),伪 App 回执先行
        let reply =
            crate::preview::handle_ctrl_request_reply(state.clone(), open_request(&path, 0, size))
                .await;
        let CtrlReply::Frame(resp) = reply else {
            panic!("open 必单帧");
        };
        assert!(resp.ok, "桥后端 open 应成: {resp:?}");
        let session = match resp.body {
            Some(ctrl_response::Body::RenditionOpen(o)) => o.session_id,
            b => panic!("open 应答体形不符: {b:?}"),
        };

        // 段读逐字节(读已产段;越界=InvalidInput 段未就绪)。回执先行
        // 的产段异步窗:段回告在途,先等记账到位(有界轮询,真 App 同窗)
        let mgr = state.rendition_sessions();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while bridge.segment_path(session, 0).is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "段回告未在窗内到达"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let mut got = Vec::new();
        mgr.segment(session, 0).unwrap().read_to_end(&mut got).unwrap();
        assert_eq!(got, b"PSEUDO-ANDROID-INIT", "init 段逐字节一致");
        got.clear();
        mgr.segment(session, 1).unwrap().read_to_end(&mut got).unwrap();
        assert_eq!(got, b"PSEUDO-ANDROID-SEG1", "内容段逐字节一致");
        let e = mgr.segment(session, 5).err().expect("未产段必 Err");
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "段未就绪词类: {e}");

        // 管线失败标记(App 0x0C ok:false):未产段即 Err(other),不再烧
        // 等待窗;已产段照读
        assert!(bridge.deliver(
            session,
            &serde_json::json!({ "error": "编码器失败", "ok": false, "request_id": session }),
        ));
        let e = mgr.segment(session, 9).err().expect("失败后未产段必 Err");
        assert_ne!(e.kind(), io::ErrorKind::InvalidInput, "失败面≠段未就绪: {e}");
        assert!(e.to_string().contains("已失败"), "失败词面透传: {e}");
        got.clear();
        mgr.segment(session, 0).unwrap().read_to_end(&mut got).unwrap();
        assert_eq!(got, b"PSEUDO-ANDROID-INIT", "已产段失败后照读");

        // 回告路径越界(fail-closed):拒读前缀外路径
        let escape = serve.path().join("evil.m4s");
        std::fs::write(&escape, b"X").unwrap();
        assert!(bridge.deliver(
            session,
            &serde_json::json!({
                "index": 2, "ok": true,
                "path": escape.to_string_lossy(), "request_id": session,
            })
        ));
        let e = mgr.segment(session, 2).err().expect("越界路径必拒读");
        assert!(e.to_string().contains("越出"), "词面: {e}");

        // cancel:会话回收 + 桥广播 fs.rendition_cancel + 段记账摘除
        assert!(mgr.cancel(session));
        // 广播各收各的:rx_cancel 队列先有 open 事件,排干到 cancel 为止
        let mut ev = rx_cancel.recv().await.unwrap();
        while ev.method != crate::jobs::EV_RENDITION_CANCEL {
            ev = rx_cancel.recv().await.unwrap();
        }
        assert_eq!(ev.params["session"], session, "会话键原样回传");
        assert_eq!(bridge.segment_path(session, 0), None, "cancel 摘段记账");
        let e = mgr.segment(session, 0).err().expect("回收后段读必 Err");
        assert_eq!(e.kind(), io::ErrorKind::NotFound, "回收词类: {e}");
    }

    /// 桥后端无 App(receiver_count = 0):open 快失败 → 工厂落 None →
    /// 稳定词 unsupported(查看端回退原流+海报;与桌面无后端同词——
    /// 查看端单分支降级,T13 工厂无错误通道的钉面)。
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bridge_backend_unreachable_app_is_unsupported() {
        let state = test_state();
        let (events, rx) =
            tokio::sync::broadcast::channel::<mtdss_ipc::EventFrame>(16);
        drop(rx); // 无订阅 = App 必然不在(快败面)
        let bridge = Arc::new(crate::jobs::RenditionBridge::with_timeout(
            events,
            Duration::from_secs(1),
        ));
        state.rendition_sessions().install_factory(super::bridge_backend(
            bridge,
            state.home().join("rendition-serve"),
        ));
        let (_dir, path, size) = make_video();
        let reply =
            crate::preview::handle_ctrl_request_reply(state, open_request(&path, 0, size)).await;
        let (ok, code, msg) = frame_msg(&reply).expect("无 App 必单帧错");
        assert_eq!((ok, code.as_str()), (false, "unsupported"), "词面: {msg}");
        assert!(msg.contains("回退"), "词面携回退语义: {msg}");
    }
}
