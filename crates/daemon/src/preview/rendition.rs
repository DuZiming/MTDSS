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
//!   出(路由实装与 seek 跳段归 T15;拉段首败 = 503 面原料,Review#1)。
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
//! 后端 `segment` 语义 = **读已产段**:实现按段粒度出流,慢产段(App
//! 转码追不上)诚实 Err(段未就绪)——T15 的「段请求缺 → 触发源端跳段,
//! 有界等待 10s → 503」在本 Err 面上收口。段读是阻塞调用,应答臂以
//! spawn_blocking 收口,流以有界通道泵(T2 帧序,纪律同 read_range 泵)。

use std::io;
use std::path::{Path, PathBuf};
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
// 测试面(查看端 Local 臂直驱分发缝)的请求构造 import。
#[cfg(test)]
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

/// 空闲回收界(Review#2;常量锚,测试注入时钟在其上断言)。
pub(crate) const RENDITION_IDLE_TTL: Duration = Duration::from_secs(60);

// ---------- 转码后端 trait + 装配缝 ----------

/// 转码后端(源端会话的背后引擎):把已就位的数据源切成 fMP4 段。实现须
/// Send+Sync(段读在 spawn_blocking 阻塞池上逐块泵);`cancel` 幂等
/// (显式 cancel/空闲回收/daemon 收口三处都走到,重复无害)。
pub(crate) trait Transcoder: Send + Sync {
    /// 取段流(index 0 = init;越界/段未就绪 = Err)。返回的读面在阻塞
    /// 池上被逐块泵读,须 Send。段尽由调用方拉至 Err 止步探测(动态
    /// playlist 的「未产段不列」面),后端无须另报总段数。
    fn segment(&self, index: u32) -> io::Result<Box<dyn io::Read + Send>>;
    /// 取消并释放后端资源(MediaCodec/VideoToolbox 管线)。
    fn cancel(&self);
}

/// 后端装配工厂:源就位后造后端(管线启动含在造面);None = 本端无转码
/// 后端(open 回稳定词 unsupported)。工厂在会话表锁**外**调用(App 桥
/// 启动可秒级,不得占表阻塞 segment/cancel)。入参 = (源面, 源 vpath)
/// ——App 桥后端(V4-1 T13)以 vpath 让 App 按 seek/thumb 桥同源分流
/// 自行开源转码(V4-1 扩:T12 原形仅源面;桥后端需要 App 侧可解析的
/// 路由键),桌面实现两参皆忽略。
pub(crate) type BackendFactory =
    Arc<dyn Fn(Arc<dyn FileSource>, &str) -> Option<Arc<dyn Transcoder>> + Send + Sync>;

/// 平台后端装配(生产面):v1 桌面全线无后端 → 恒 None(open 回
/// unsupported,查看端回退原流+海报);Android 真后端(V4-1 T13
/// MediaCodec 桥)与 iOS 真后端(V4-1 T14 AVAssetReader+VideoToolbox 桥,
/// 同一 0x0C 线形)由 Daemon::start 经
/// [`RenditionSessionMgr::install_factory`] 挂入([`bridge_backend`])。
pub(crate) fn platform_backend() -> BackendFactory {
    Arc::new(|_source, _path| None)
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
/// 驱动 Kotlin MediaCodec 管线——工厂调用 = 开桥会话(start_at=0;seek 的
/// 「起点段」重开会话由 T15 查看端以新 open 承载),成功 = 会话键(session
/// = 桥 request_id)铸 [`AndroidTranscoder`];**桥不可用/超时/拒答一律落
/// None**(工厂缝无错误通道,T12 钉死)→ open 回 unsupported → 查看端回
/// 退原流+海报(降级面诚实:失败词面走 tracing 日志消费)。工厂闭包在
/// 会话表锁外、spawn_blocking 阻塞池上调用(评审 Important #1 同纪律),
/// 内部 `Handle::block_on` 驱动桥的异步 open(oneshot 有界等待,不占异步
/// worker)。unix 门:fdpass 通道面(Windows 编译单元整体缺席,占位桥
/// 恒 Err,本工厂不装配)。
#[cfg(unix)]
pub(crate) fn bridge_backend(
    bridge: std::sync::Arc<crate::jobs::RenditionBridge>,
    serve_dir: PathBuf,
) -> BackendFactory {
    Arc::new(move |source, path| {
        let opened = tokio::runtime::Handle::try_current()
            .ok()
            .and_then(|h| h.block_on(bridge.open(path, 0)).ok());
        match opened {
            Some(session) => {
                // 源面随手柄落放(桥后端不经 FileSource 读源——App 按
                // vpath 自行开源转码;source 入参仅供签名一致性/桌面实现)
                drop(source);
                Some(Arc::new(AndroidTranscoder {
                    bridge: Arc::clone(&bridge),
                    session,
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
/// 段未回告 = InvalidInput「段未就绪」——T15 有界等待/503 在本 Err 面
/// 收口;管线失败标记在场时新段即 Err 勿烧等待);段文件生命周期归 App
/// (cancel 即清,引擎只读不删——重复拉段/重试照读)。cancel = 桥发后
/// 即忘广播(App 停管线+清段文件)。
#[cfg(unix)]
struct AndroidTranscoder {
    bridge: std::sync::Arc<crate::jobs::RenditionBridge>,
    session: u64,
    serve_dir: PathBuf,
}

#[cfg(unix)]
impl Transcoder for AndroidTranscoder {
    fn segment(&self, index: u32) -> io::Result<Box<dyn io::Read + Send>> {
        let path = match self.bridge.segment_path(self.session, index) {
            Some(p) => p,
            None if self.bridge.failure(self.session).is_some() => {
                // 管线已失败:未产段必不再来,即 Err 勿烧 T15 等待窗
                let why = self.bridge.failure(self.session).unwrap_or_default();
                return Err(io::Error::other(format!("转码管线已失败(段 {index} 未就绪): {why}")));
            }
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("段未就绪(生产中或越界): {index}"),
                ))
            }
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
    pub(crate) fn open(
        &self,
        source: Arc<dyn FileSource>,
        config: &RenditionConfig,
        path: &str,
    ) -> Result<u64, OpenError> {
        self.sweep();
        let factory = Arc::clone(&self.factory.lock().unwrap());
        let backend = (factory)(source, path).ok_or(OpenError::Unsupported)?;
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
        Ok(id)
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
    let sessions = state.rendition_sessions();
    let path = r.path.clone();
    let opened =
        tokio::task::spawn_blocking(move || {
            sessions.open(source, &RenditionConfig::DEFAULT, &path)
        })
        .await;
    match opened {
        Ok(Ok(session_id)) => CtrlReply::Frame(CtrlResponse {
            version: mtdss_proto::CTRL_VERSION,
            ok: true,
            body: Some(ctrl_response::Body::RenditionOpen(RenditionOpenResponse {
                session_id,
                segment_ms: RenditionConfig::SEGMENT_MS,
                mime: SEGMENT_MIME.into(),
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

/// 段取错误 → 稳定词单帧:会话不存在/已回收 = NOT_FOUND(查看端重开);
/// 越界/段未就绪 = NOT_FOUND 携「段越界」词(动态 playlist 拉至 Err 止
/// 步的探测面);其余 = source_offline(T15 的 503 面原料)。
fn segment_err_response(e: &io::Error) -> CtrlResponse {
    match e.kind() {
        io::ErrorKind::NotFound => err_response(
            "NOT_FOUND",
            format!("转码会话不存在或已回收(请重新 open): {e}"),
        ),
        io::ErrorKind::InvalidInput => {
            err_response("NOT_FOUND", format!("段越界(或段未就绪): {e}"))
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
/// 消费面 = T15(loopback video 路由);本任务先落装配骨架+单测
/// (brief 钉死),生产臂 T15 挂入——按需保留 allow(沿 house 口径)。
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug)]
pub(crate) enum AttachError {
    /// 源端无转码后端(稳定词 `unsupported`)。
    Unsupported,
    /// 其余失败(稳定码,文案;源离线/NOT_FOUND 等)。
    Failed(String, String),
}

/// 段获取面(查看端装配的源通道):生产 = ctrl 池往返(远端设备);测试
/// = 进程内直驱源端分发缝(免网络面,伪转码器全链路)。错误归一
/// (稳定码,文案),消费面按稳定词分流(unsupported → 回退;NOT_FOUND
/// → 重开/止步)。消费面 = T15(骨架先行,同 [`AttachError`] 注)。
pub(crate) enum RenditionFetch {
    /// 远端设备:Weak 防 state↔装配引用环(RemoteFileSource 先例)。
    /// (测试台只用 Local 直驱面;生产构造随 T15 生产臂挂入。)
    #[allow(dead_code)] // T15 消费(骨架先行)
    Remote {
        state: std::sync::Weak<DaemonState>,
        device: String,
    },
    /// 进程内源端(测试):直驱 [`super::handle_ctrl_request_reply`]。
    #[cfg(test)]
    Local(Arc<DaemonState>),
}

impl RenditionFetch {
    /// 生产构造(远端设备)。
    #[allow(dead_code)] // T15 消费(骨架先行)
    pub(crate) fn remote(state: &Arc<DaemonState>, device: &str) -> Self {
        Self::Remote {
            state: Arc::downgrade(state),
            device: device.to_string(),
        }
    }

    /// 测试构造(进程内源端)。
    #[cfg(test)]
    fn local(state: Arc<DaemonState>) -> Self {
        Self::Local(state)
    }

    async fn open(
        &self,
        path: &str,
        mtime: i64,
        size: u64,
    ) -> Result<RenditionOpenResponse, (String, String)> {
        let req = RenditionOpenRequest {
            path: path.to_string(),
            mtime,
            size: size as i64,
            target: mtdss_proto::ctrl::RenditionTarget::Hls as i32,
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
            #[cfg(test)]
            Self::Local(src) => {
                let creq = CtrlRequest {
                    version: mtdss_proto::CTRL_VERSION,
                    body: Some(ctrl_request::Body::RenditionOpen(req)),
                };
                match super::handle_ctrl_request_reply(Arc::clone(src), creq).await {
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
            #[cfg(test)]
            Self::Local(src) => {
                let creq = CtrlRequest {
                    version: mtdss_proto::CTRL_VERSION,
                    body: Some(ctrl_request::Body::RenditionSegment(
                        RenditionSegmentRequest { session_id, index },
                    )),
                };
                match super::handle_ctrl_request_reply(Arc::clone(src), creq).await {
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
            #[cfg(test)]
            Self::Local(src) => {
                let creq = CtrlRequest {
                    version: mtdss_proto::CTRL_VERSION,
                    body: Some(ctrl_request::Body::RenditionCancel(RenditionCancel {
                        session_id,
                    })),
                };
                match super::handle_ctrl_request_reply(Arc::clone(src), creq).await {
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
#[cfg_attr(not(test), allow(dead_code))] // 消费面 = T15(骨架先行)
fn frame_error(resp: &CtrlResponse) -> Option<(String, String)> {
    resp.body.as_ref().and_then(|b| match b {
        ctrl_response::Body::Error(e) => Some((e.code.clone(), e.message.clone())),
        _ => None,
    })
}

/// 流式收齐(测试 Local 面:通道项即生产块)。
#[cfg(test)]
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
#[cfg_attr(not(test), allow(dead_code))] // 消费面 = T15(骨架先行)
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

/// 查看端转码装配(每视频一份):持有源端会话与已拉段账;段字节落 T5
/// 缓存(`{home}/preview-tmp`,键 = `{prefix}vseg{i}`,prefix 由回环
/// handle 铸,T15 的 /f/{h}/video/ 路由消费本面)。骨架先行(brief 钉死
/// 单测面),生产臂 T15 挂入——按需保留 allow(沿 house 口径)。
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct ViewerRendition {
    fetch: RenditionFetch,
    session_id: u64,
    segment_ms: u32,
    mime: String,
    /// 已拉段数(含 init 段 0;连续无洞前缀——pull 逐段推进,一败即止)。
    pulled: u32,
}

#[cfg_attr(not(test), allow(dead_code))]
impl ViewerRendition {
    /// 装配:rendition_open → 会话元信息。Err(Unsupported) = 源端无后端
    /// → 调用方回退原流+海报(T15 面);Err(Failed) = 其余失败。
    pub(crate) async fn attach(
        fetch: RenditionFetch,
        path: &str,
        mtime: i64,
        size: u64,
    ) -> Result<Self, AttachError> {
        let resp = fetch.open(path, mtime, size).await.map_err(|(code, msg)| {
            if code == "unsupported" {
                AttachError::Unsupported
            } else {
                AttachError::Failed(code, msg)
            }
        })?;
        Ok(Self {
            fetch,
            session_id: resp.session_id,
            segment_ms: resp.segment_ms,
            mime: resp.mime,
            pulled: 0,
        })
    }

    /// 源端会话 id(测试/T15 缝:背会话拆台面)。
    pub(crate) fn session_id(&self) -> u64 {
        self.session_id
    }

    /// 段 MIME(open 应答透传;T15 loopback 段应答 Content-Type 面)。
    #[allow(dead_code)] // 消费面 = T15 段路由
    pub(crate) fn mime(&self) -> &str {
        &self.mime
    }

    /// 拉段入 T5 缓存(段 0 init 起,逐段推进到首个 Err 为止):首段
    /// (init)拉不到 = Err(Review#1:loopback 503 面的原料);已拉段
    /// 保留(部分内容可播;T15 续拉/跳段在其上收口)。返回已拉段数
    /// (含 init)。
    pub(crate) async fn pull_segments(
        &mut self,
        cache: &super::cache::PreviewDiskCache,
        key_prefix: &str,
    ) -> Result<u32, (String, String)> {
        let mut index = self.pulled;
        loop {
            match self.fetch.segment(self.session_id, index).await {
                Ok(bytes) => {
                    cache
                        .store(
                            &seg_cache_key(key_prefix, index),
                            &mut io::Cursor::new(&bytes[..]),
                        )
                        .map_err(|e| ("source_offline".into(), format!("段缓存落盘失败: {e}")))?;
                    self.pulled += 1;
                    index = index.saturating_add(1);
                }
                Err(e) => {
                    if index == 0 {
                        return Err(e); // init 拉不到 = 503 面
                    }
                    break; // 越界/会话收尾 = 正常止(段尽探测)
                }
            }
        }
        Ok(self.pulled)
    }
    /// 动态 HLS playlist(已拉段列出,未拉段不列;`#EXT-X-VERSION:7`/
    /// fMP4/`#EXT-X-MAP`;相对 URI 对 /f/{h}/video/ 落 T15 的 seg 路由)。
    /// 终未到不写 `#EXT-X-ENDLIST`(EVENT 形;T15 seek/续拉语义在其上
    /// 生长)。pulled = 0(仅 attach 未拉段)= 纯头形。
    pub(crate) fn playlist_m3u8(&self) -> String {
        let target = self.segment_ms.div_ceil(1000).max(1);
        let secs = self.segment_ms as f64 / 1000.0;
        let mut s = format!(
            "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-PLAYLIST-TYPE:EVENT\n#EXT-X-TARGETDURATION:{target}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-MAP:URI=\"seg/0.m4s\"\n"
        );
        for i in 1..self.pulled {
            s.push_str(&format!("#EXTINF:{secs:.3},\nseg/{i}.m4s\n"));
        }
        s
    }

    /// 段缓存件路径(T15 loopback 面:命中即回缓存件;未拉段 = None)。
    #[allow(dead_code)] // 消费面 = T15 段路由
    pub(crate) fn segment_path(
        &self,
        cache: &super::cache::PreviewDiskCache,
        key_prefix: &str,
        index: u32,
    ) -> Option<PathBuf> {
        if index < self.pulled {
            cache.path_for(&seg_cache_key(key_prefix, index))
        } else {
            None
        }
    }

    /// 拆卸:cancel 源端会话(App 侧转码资源即释)。幂等(未知会话照成)。
    pub(crate) async fn detach(self) {
        let _ = self.fetch.cancel(self.session_id).await;
    }
}

// ---------- 测试后端(伪转码器)+ 测试面 ----------

/// 伪转码器(cfg(test)):init + 2 内容段(共 3 个可拉段)已知字节,
/// 越界 Err(InvalidInput);cancel 计数可断言(brief「3 段已知字节」
/// 测试面:open→seg0..2 逐字节;越界→Err;cancel 后 seg→Err;60s 回收)。
#[cfg(test)]
struct PseudoTranscoder {
    cancels: std::sync::atomic::AtomicU32,
}

#[cfg(test)]
impl PseudoTranscoder {
    /// init 段已知字节(段 0;EXT-X-MAP 面)。
    fn init_bytes() -> Vec<u8> {
        b"PSEUDO-FMP4-INIT-HEADER-0".to_vec()
    }

    /// 内容段已知字节(段 1..=2;长度随序可复算,泵多块面不在此造——
    /// 段字节远小于 READ_CHUNK,单块到齐)。
    fn segment_bytes(index: u32) -> Vec<u8> {
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
    fn segment(&self, index: u32) -> io::Result<Box<dyn io::Read + Send>> {
        match index {
            0 => Ok(Box::new(io::Cursor::new(Self::init_bytes()))),
            1..=2 => Ok(Box::new(io::Cursor::new(Self::segment_bytes(index)))),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("段越界: {index}"),
            )),
        }
    }

    fn cancel(&self) {
        self.cancels
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// 伪转码器工厂(测试面):每 open 造一枚,实例收进共享槽(cancel
/// 计数断言面)。vpath 入参照录不消费(桌面伪器无 App 桥面)。
#[cfg(test)]
fn pseudo_factory() -> (BackendFactory, Arc<Mutex<Vec<Arc<PseudoTranscoder>>>>) {
    let made = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&made);
    let factory: BackendFactory = Arc::new(move |_src, _path| {
        let t = Arc::new(PseudoTranscoder {
            cancels: std::sync::atomic::AtomicU32::new(0),
        });
        sink.lock().unwrap().push(Arc::clone(&t));
        Some(t as Arc<dyn Transcoder>)
    });
    (factory, made)
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

    /// 请求构造小助(三 op)。
    fn open_request(path: &str, mtime: i64, size: u64) -> CtrlRequest {
        CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: Some(Req::RenditionOpen(RenditionOpenRequest {
                path: path.to_string(),
                mtime,
                size: size as i64,
                target: RenditionTarget::Hls as i32,
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

        // 越界 index 3 → NOT_FOUND 单帧(词面携「越界」,与「会话已回收」可区分)
        let reply = crate::preview::handle_ctrl_request_reply(
            state.clone(),
            seg_request(open.session_id, 3),
        )
        .await;
        let (ok, code, msg) = frame_msg(&reply).expect("越界必单帧错");
        assert_eq!((ok, code.as_str()), (false, "NOT_FOUND"));
        assert!(msg.contains("越界"), "越界词面: {msg}");
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

        let sid = mgr
            .open(
                Arc::new(MemSource {
                    data: vec![0u8; 64],
                }),
                &RenditionConfig::DEFAULT,
                "/收件箱/movie.bin",
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

        // 越界(活会话)= InvalidInput,与回收 NotFound 可区分
        let sid2 = mgr
            .open(
                Arc::new(MemSource {
                    data: vec![0u8; 64],
                }),
                &RenditionConfig::DEFAULT,
                "/收件箱/movie.bin",
            )
            .unwrap();
        let e = mgr.segment(sid2, 9).err().expect("活会话越界段读必 Err");
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "越界词: {e}");

        // 活动刷新续命:摸过即重计(30s 摸 → 再 30s 仍活;再 60s 即回收)
        *clock.lock().unwrap() += Duration::from_secs(30);
        assert!(mgr.segment(sid2, 0).is_ok(), "30s 摸一次");
        *clock.lock().unwrap() += Duration::from_secs(30);
        assert!(mgr.segment(sid2, 0).is_ok(), "累计 60s 但真空闲 30s:仍活");
        *clock.lock().unwrap() += Duration::from_secs(60);
        assert!(mgr.segment(sid2, 0).is_err(), "真空闲 60s:回收");

        // 显式 cancel 后段读 NotFound
        let sid3 = mgr
            .open(
                Arc::new(MemSource {
                    data: vec![0u8; 64],
                }),
                &RenditionConfig::DEFAULT,
                "/收件箱/movie.bin",
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
        let factory: BackendFactory = Arc::new(move |_src, _path| {
            entered_f.store(true, AtomicOrdering::SeqCst);
            std::thread::sleep(Duration::from_millis(200));
            Some(Arc::new(PseudoTranscoder {
                cancels: AtomicU32::new(0),
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

        let fetch = RenditionFetch::local(Arc::clone(&source));
        let mut v = ViewerRendition::attach(fetch, &path, 0, size)
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

        // playlist 形状(brief:#EXT-X-VERSION:7 / fMP4 / #EXT-X-MAP)
        let pl = v.playlist_m3u8();
        assert!(pl.starts_with("#EXTM3U\n"), "M3U8 头: {pl}");
        assert!(pl.contains("#EXT-X-VERSION:7"), "版本 7: {pl}");
        assert!(pl.contains("#EXT-X-PLAYLIST-TYPE:EVENT"), "动态 EVENT 形");
        assert!(pl.contains("#EXT-X-TARGETDURATION:4"), "4000ms → 4s");
        assert!(pl.contains("#EXT-X-MAP:URI=\"seg/0.m4s\""), "fMP4 MAP 面");
        assert!(pl.contains("#EXTINF:4.000,\nseg/1.m4s\n"), "内容段 1: {pl}");
        assert!(pl.contains("seg/2.m4s"), "内容段 2");
        assert!(!pl.contains("#EXT-X-ENDLIST"), "终未到不写 ENDLIST");

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
        let fetch = RenditionFetch::local(Arc::clone(&source));
        let err = ViewerRendition::attach(fetch, &path, 0, size)
            .await
            .err()
            .expect("无后端 attach 必 Err");
        assert!(
            matches!(err, AttachError::Unsupported),
            "无后端 = Unsupported: {err:?}"
        );
        let fetch = RenditionFetch::local(source);
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
        let fetch = RenditionFetch::local(Arc::clone(&source));
        let mut v = ViewerRendition::attach(fetch, &path, 0, size)
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
