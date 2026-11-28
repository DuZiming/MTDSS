//! 预览回环 HTTP 服务(V4-1 T4):仅绑 127.0.0.1 随机口的本地数据面。
//!
//! 路由全景(brief 钉死):`GET /f/{h}/raw`(Range → 206/200/416)、
//! `/f/{h}/thumb?w=`(缩略图透传,T7:jpeg bytes 或空 200)、`/f/{h}/jpeg`
//! (HEIC 转码,T6;非 Windows 404 占位)、`/f/{h}/video/index.m3u8`
//! (动态 playlist,T15:MIME `application/vnd.apple.mpegurl`;已缓存段
//! 列出,未拉段不列,段序无洞;**段 URI 恒绝对+携 `?t=` 查询令牌**——
//! T21 fix round 1 Critical #1:相对 URI 经 hls.js RFC 3986 解析丢查询串
//! → 段请求 401,单点于 playlist 生成面绝对化)与 `/f/{h}/video/seg/{n}.m4s`
//! (段面,T15:
//! 命中 = 缓存件读盘直出(M3 消费时过存在性);未拉 = 跳段直取目标段
//! (乱序直达源端,有界等待 10s → 503;窗过半仍未达 = 远跳重开换道,
//! T15b:cancel+携 start_at 重开+余窗续等,503 携「重试」稳定词 +
//! Retry-After;源离线 = 503 + 稳定词;越界 = 404 止步词)。视频装配
//! ([`super::rendition::ViewerRendition`])随
//! handle 惰性 attach,`preview_close`/路由 410 面即 detach(spec §7
//! 「关预览=会话随 handle 释放」;表在 [`PreviewGate`],IPC 关闭面与路由
//! 共享)。鉴权双缝(V4-1 T20 裁定②)`Authorization: Bearer <token>` 或
//! `?t=<token>` 查询参数等价(后者供 previewer bundle/`<img>`/`<video>`
//! 等无法携头的回环 GET 消费面;token 经 preview_open IPC 交付本机 UI,
//! spec §2)缺/错 = 401,**先于**一切路由/handle 判定。
//! handle 未知/过期/已关 = 410(Gone;不区分「从无」与「已失效」,不探
//! 测存在性)。Range 非法矩阵:off ≥ size / len=0(suffix 0)= 416(携
//! `Content-Range: bytes */size`);畸形 header(非 bytes= 单位/多段/
//! 逆序/非数字)= 400。
//!
//! 数据面:块对齐**内存预读 LRU**([`Readahead`],窗口 [`READAHEAD_BLOCK`]
//! = 2MiB,总帽 [`READAHEAD_CAP_BYTES`] = 64MiB;同一 raw 连续 range 落同
//! 块即窗口命中,免重读源)→ 有界流式应答(读源走 spawn_blocking,fs 面
//! 同款纪律);首块在应答头之前装货(首读失败 = 503,客户端回退/重试;
//! 流中失败 = 截断收尾,HTTP 语义内诚实降级)。close/空闲回收后:**新请
//! 求 = 410,在途流逐块检 closed 旗标即停**(「关面板即断流」,spec §7)。
//!
//! 生命周期:全 daemon 单例([`PreviewGate`] 惰性起——首枚 preview_open
//! 才占口;bind 127.0.0.1:0 失败换口重试);token 每实例一枚(handle 经
//! IPC 出,均不出本机)。

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;

use super::handles::HandleRegistry;
use super::source::FileSource;

/// 预读窗口块长(spec §2「预读窗口 2 MiB」;常量锚,改值须过测试)。
pub(crate) const READAHEAD_BLOCK: u64 = 2 * 1024 * 1024;

/// 块读防抖重试帽(r44):中继瞬断/连接抖动自愈窗——1s 退避 ×N;耗尽才
/// 诚实截断。常量锚,改值须过测试。
const RAW_BLOCK_RETRY_MAX: u32 = 3;
/// 内存预读 LRU 总帽(spec §2「LRU 缓存上限 64 MiB」;只管流式预读,与
/// T5 落盘缓存分账)。
pub(crate) const READAHEAD_CAP_BYTES: usize = 64 * 1024 * 1024;
/// /jpeg 整取读入帽(V4-1 T6 评审 Important #1):整取**先按 size 快照
/// 拒**,远端把 /jpeg 指向巨型文件时不进 RAM(HEIC 照片 2-10MB 量级,
/// 256MB ≈ 40 倍极端全景余量;超帽恒 422,客户端回退 raw)。
#[cfg(all(windows, feature = "heic"))]
const JPEG_SOURCE_CAP_BYTES: u64 = 256 * 1024 * 1024;
/// bind 随机口重试次数(port=0 每次 OS 换口;连续失败 = 本机 TCP 栈异常)。
const BIND_ATTEMPTS: usize = 8;

// ---------- Range 解析(纯函数,单测钉矩阵) ----------

/// Range header 解析结果(对 size 归一后的闭区间段)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RangeOutcome {
    /// 无 Range header → 200 全量。
    Full,
    /// 有合法单段 → 206 `[start, end]`(闭区间;end 已按 size 收口)。
    Part { start: u64, end: u64 },
}

/// Range header 非法形态(映射:Malformed → 400;Unsatisfiable → 416)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RangeErr {
    /// 畸形:非 `bytes=` 单位、多段(本实现不支持 multipart)、start>end、
    /// 非数字、溢出。
    Malformed,
    /// 语法合法但不可满足:start ≥ size,或 suffix len=0。
    Unsatisfiable,
}

/// `Range: bytes=...` 单段解析(RFC 7233 单段面;多段/他单位按畸形拒,
/// 客户端面全是我们自己的 previewer/原生播放器,不带 multipart 诉求)。
pub(crate) fn parse_range(
    header: Option<&HeaderValue>,
    size: u64,
) -> Result<RangeOutcome, RangeErr> {
    let Some(hv) = header else {
        return Ok(RangeOutcome::Full);
    };
    let Ok(spec) = hv.to_str() else {
        return Err(RangeErr::Malformed);
    };
    let Some(value) = spec.strip_prefix("bytes=") else {
        return Err(RangeErr::Malformed);
    };
    // 多段(逗号)= 畸形(单段实现,见上注)
    if value.contains(',') {
        return Err(RangeErr::Malformed);
    }
    let bad_num = |s: &str| -> Option<u64> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        s.parse::<u64>().ok()
    };
    let out = if let Some((start, end)) = value.split_once('-') {
        match (bad_num(start), bad_num(end)) {
            // `a-b`:双数字闭区间(end 越界收口到源末;a>b = 畸形)
            (Some(a), Some(b)) => {
                if a > b {
                    return Err(RangeErr::Malformed);
                }
                if a >= size {
                    return Err(RangeErr::Unsatisfiable);
                }
                RangeOutcome::Part {
                    start: a,
                    end: b.min(size - 1),
                }
            }
            // `a-`:开放尾(off ≥ size = 不可满足)
            (Some(a), None) => {
                if a >= size {
                    return Err(RangeErr::Unsatisfiable);
                }
                RangeOutcome::Part {
                    start: a,
                    end: size - 1,
                }
            }
            // `-n`:suffix(n=0 = len 0 = 不可满足;超长 = 全量段)
            (None, Some(n)) => {
                if n == 0 {
                    return Err(RangeErr::Unsatisfiable);
                }
                let start = size.saturating_sub(n);
                RangeOutcome::Part {
                    start,
                    end: size - 1,
                }
            }
            // `-` / 缺臂非数字 = 畸形
            _ => return Err(RangeErr::Malformed),
        }
    } else {
        return Err(RangeErr::Malformed);
    };
    Ok(out)
}

// ---------- 内存预读 LRU ----------

/// 缓存块键(handle 条目 id × 块序;id 单调不复用,close 后旧块不可达,
/// 由 LRU 自然出清——无逐 handle 主动失效面)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    entry: u64,
    block: u64,
}

struct CacheLine {
    data: Arc<[u8]>,
    stamp: u64,
}

#[derive(Default)]
struct CacheState {
    map: HashMap<CacheKey, CacheLine>,
    bytes: usize,
    stamp: u64,
}

/// 块对齐内存预读 LRU(全 server 共享一份;std 锁不跨 await——命中即返,
/// 未命中的源读在锁外 spawn_blocking)。
pub(crate) struct Readahead {
    inner: Mutex<CacheState>,
    cap: usize,
}

impl Readahead {
    /// 生产构造(常量帽 [`READAHEAD_CAP_BYTES`])。
    pub(crate) fn new() -> Self {
        Self::with_cap(READAHEAD_CAP_BYTES)
    }

    /// 帽注入构造(生产 `new` 与测试面共用)。
    fn with_cap(cap: usize) -> Self {
        Self {
            inner: Mutex::new(CacheState::default()),
            cap,
        }
    }

    /// 取块(窗口命中):在场即返并刷新 LRU 时序;缺席从源读整块(尾部
    /// 短块 = 至源末)入缓存,超帽按最旧出清。`block` 须 `start < size`。
    pub(crate) async fn block(
        &self,
        entry: u64,
        block: u64,
        source: Arc<dyn FileSource>,
        size: u64,
    ) -> io::Result<Arc<[u8]>> {
        let key = CacheKey { entry, block };
        {
            let mut st = self.inner.lock().unwrap();
            st.stamp += 1;
            let now = st.stamp;
            if let Some(line) = st.map.get_mut(&key) {
                line.stamp = now;
                return Ok(Arc::clone(&line.data));
            }
        }
        // 锁外读源(spawn_blocking:fs seek+read 阻塞,blocking_fs 同款纪律)
        let start = key
            .block
            .checked_mul(READAHEAD_BLOCK)
            .ok_or_else(|| io::Error::other("块序溢出"))?;
        debug_assert!(start < size, "调用方保证块起点落源内");
        let len = (size - start).min(READAHEAD_BLOCK) as usize;
        let data = tokio::task::spawn_blocking(move || {
            let mut buf = vec![0u8; len];
            let mut off = 0usize;
            while off < len {
                let n = source.read_at(start + off as u64, &mut buf[off..])?;
                if n == 0 {
                    break; // EOF(短块余量诚实呈现)
                }
                off += n;
            }
            buf.truncate(off);
            Ok::<_, io::Error>(buf)
        })
        .await
        .map_err(|e| io::Error::other(format!("读源任务收尾失败: {e}")))??;
        let data: Arc<[u8]> = Arc::from(data.into_boxed_slice());
        let mut st = self.inner.lock().unwrap();
        st.stamp += 1;
        let stamp = st.stamp;
        // 入缓存 + 按字节帽出清最旧(帽 ≤ 64MiB = 32 块,O(n) 扫描无感)
        st.map.insert(
            key,
            CacheLine {
                data: Arc::clone(&data),
                stamp,
            },
        );
        st.bytes += data.len();
        while st.bytes > self.cap {
            let Some(oldest) = st.map.iter().min_by_key(|(_, l)| l.stamp).map(|(k, _)| *k) else {
                break;
            };
            if let Some(line) = st.map.remove(&oldest) {
                st.bytes -= line.data.len();
            }
        }
        Ok(data)
    }

    /// 缓存块数(单测锚)。
    #[cfg(test)]
    fn len(&self) -> usize {
        self.inner.lock().unwrap().map.len()
    }
}

// ---------- 服务本体 ----------

/// 回环服务上下文(路由 state)。
struct SrvCtx {
    registry: Arc<HandleRegistry>,
    token: String,
    /// 监听口(bind 后回填;消费面 = video playlist 段 URI 绝对化拼装——
    /// T21 fix round 1 Critical #1,见 rendition::playlist_m3u8 注)。
    port: u16,
    cache: Arc<Readahead>,
    /// 转码结果落盘缓存(T5;None = 未接线——缺缓存只损失命中面,转码
    /// 照常。类型平台无关,非 Windows / 未启用 heic 时不消费)。
    #[cfg_attr(
        not(all(windows, feature = "heic")),
        allow(dead_code) // 占位构建:消费面(/jpeg 缓存命中)未编译
    )]
    disk: Option<Arc<super::cache::PreviewDiskCache>>,
    /// daemon state 弱引用(V4-1 T7,/thumb 面):本机臂取缩略图缓存、
    /// 远端臂取 ctrl 池转发;Weak 防 state↔gate↔server 引用环
    /// (RemoteFileSource 同款)。None = 未注入(测试缺省面)——thumb
    /// 恒空字节回退,不损其余路由。
    state: Option<std::sync::Weak<crate::state::DaemonState>>,
    /// 视频装配表(V4-1 T15;gate 共享单份,见 [`PreviewGate::video`] 注
    /// ——路由惰性 attach,close/410 面即摘)。
    video: super::rendition::VideoSessions,
}

/// 全 daemon 单例门:惰性起服务(首枚 preview_open 才 bind),重复取用
/// 同一实例。tokio OnceCell 持锁跨 await(仅首启),取用无锁。
pub(crate) struct PreviewGate {
    registry: Arc<HandleRegistry>,
    /// 转码落盘缓存(T5;state.rs 装配注入——缓存构造需要注册表句柄,
    /// 「先门后缓存」的构造序决定走注入面;首枚 preview_open 前必已注入,
    /// 未注入(None)只损失 /jpeg 缓存命中,不损正确性)。
    disk: std::sync::OnceLock<Arc<super::cache::PreviewDiskCache>>,
    /// daemon state 弱引用(V4-1 T7;/thumb 本机引擎臂与远端 ctrl 转发臂
    /// 消费;lib.rs 在 state 成 Arc 后注入——load 时 Arc 尚未成形;Weak
    /// 防 state↔gate↔server 引用环)。未注入 = /thumb 恒空字节(测试缺省
    /// 面),不损其余路由。
    state: std::sync::OnceLock<std::sync::Weak<crate::state::DaemonState>>,
    /// 视频装配表(V4-1 T15;handle → 查看端转码装配):路由惰性 attach,
    /// preview_close 与视频路由 410 面即摘 + 后台 detach(源端会话随
    /// handle 释放,spec §7「关预览=会话随 handle 释放」——手机端编码器
    /// 随面板关停,不等源端 60s 空闲回收;UI 崩溃漏 close 的残项由源端
    /// 空闲回收兜底)。单份经 [`Self::video_sessions`] 供 server 路由共享。
    video: super::rendition::VideoSessions,
    server: tokio::sync::OnceCell<Arc<PreviewServer>>,
}

impl PreviewGate {
    pub(crate) fn new() -> Self {
        Self {
            registry: Arc::new(HandleRegistry::new()),
            disk: std::sync::OnceLock::new(),
            state: std::sync::OnceLock::new(),
            video: Arc::new(Mutex::new(HashMap::new())),
            server: tokio::sync::OnceCell::new(),
        }
    }

    /// 注册表句柄(preview_close 直接关,无需起服务)。
    pub(crate) fn registry(&self) -> Arc<HandleRegistry> {
        Arc::clone(&self.registry)
    }

    /// 注入 T5 落盘缓存(state.rs 装配面;/jpeg 缓存命中消费)。重复注入
    /// 取首枚(幂等;OnceLock 单向)。
    pub(crate) fn set_disk_cache(&self, disk: Arc<super::cache::PreviewDiskCache>) {
        let _ = self.disk.set(disk);
    }

    /// 注入 daemon state 弱引用(lib.rs 装配面;/thumb 面消费)。重复注入
    /// 取首枚(幂等;OnceLock 单向)。
    pub(crate) fn set_state(&self, state: std::sync::Weak<crate::state::DaemonState>) {
        let _ = self.state.set(state);
    }

    /// 视频装配表句柄(server 路由面共享同一份;见 [`Self::video`] 注)。
    pub(crate) fn video_sessions(&self) -> super::rendition::VideoSessions {
        Arc::clone(&self.video)
    }

    /// 摘视频装配 + 后台 detach(源端会话 cancel——App 侧转码管线随面板
    /// 关停;spec §7)。消费面 = api preview_close 与视频路由 410 面。
    /// 幂等(无装配即 no-op)。
    pub(crate) fn detach_video(&self, handle: &str) {
        let Some(v) = self.video.lock().unwrap().remove(handle) else {
            return;
        };
        tokio::spawn(async move { v.detach().await });
    }

    /// 服务就绪(惰性首启;失败上抛,IPC 面映业务错)。
    pub(crate) async fn server(&self) -> io::Result<Arc<PreviewServer>> {
        let disk = self.disk.get().cloned();
        let state = self.state.get().cloned();
        let video = self.video_sessions();
        self.server
            .get_or_try_init(|| {
                PreviewServer::start(Arc::clone(&self.registry), disk, state, video)
            })
            .await
            .cloned()
    }

    /// 收口(评审 Important #2;消费面 = `Daemon::shutdown`):已启服务即
    /// abort serve/清扫两任务——监听释放、任务不留泄漏。未启过 = 纯 no-op
    /// (OnceCell 空,本就无任务)。收口后再取用返回已断的旧实例:shutdown
    /// 之后 IPC 服务同在收尾,无客户端可达面(语义注,不做槽位重置——
    /// OnceCell 首启即定,「重启新口」属新 DaemonState 的新门)。
    pub(crate) fn shutdown(&self) {
        if let Some(srv) = self.server.get() {
            srv.abort_tasks();
        }
    }
}

/// 起好的回环服务:port/token 经 preview_open IPC 交付本机 UI(注册表与
/// 预读缓存都活在路由 state [`SrvCtx`],本结构只是凭据投影);**r54 #3
/// 监督自愈**——serve 任务退出(挂起/恢复期 fd 异常等任何致因)即自动
/// 重绑新口新令牌并更新凭据投影,preview_open 下一次调用即发新口:
/// 「后台/锁屏后预览全灭且不自愈(唯重启 App 可救)」的永久瘫形态根修
/// (此前 serve 一退 = 端口永死,首启即定无自愈面)。
pub(crate) struct PreviewServer {
    /// 现行凭据(重绑即换;preview_open 读现值)。
    creds: Arc<std::sync::RwLock<LoopbackCreds>>,
    /// 快路重绑唤醒(r55 #3:懒健康检查/看门狗/测试击杀共道——abort
    /// 当前代并**跳过退避**立即重绑;「挂而不死」形态的自愈入口)。
    rebind: Arc<tokio::sync::Notify>,
    /// 收口旗(收口置位;监督环见旗即退,不再重绑)。
    closed: Arc<std::sync::atomic::AtomicBool>,
    /// 收口广播(watch:当前 serve 代与监督环各持收端,send(true) 即
    /// 全员自终——serve 代 drop listener,监督环退出不再重绑;比 Notify
    /// 单许可竞态面干净)。
    shutdown: Arc<tokio::sync::watch::Sender<bool>>,
    /// 监督环任务(serve 终局 → 退避/快路重绑循环)。
    supervisor: tokio::task::JoinHandle<()>,
    /// 周期看门狗任务(5s 自连;失联 → 快路重绑)。
    watchdog: tokio::task::JoinHandle<()>,
    /// 清扫任务句柄(见 [`HandleRegistry::spawn_sweeper`])。
    sweeper: tokio::task::JoinHandle<()>,
}

/// 回环凭据(端口 + 令牌;每代绑定一枚新令牌——旧 URL 随旧口同死,
/// 现行取数一律经 preview_open 取现值)。
struct LoopbackCreds {
    port: u16,
    token: String,
}

/// 端口活性自检(本机 TCP 连 100ms 超时;监听不在 = false)。
async fn port_alive(port: u16) -> bool {
    tokio::time::timeout(
        std::time::Duration::from_millis(100),
        tokio::net::TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    .is_ok_and(|r| r.is_ok())
}

/// 单代 serve 任务:axum::serve + 收口广播自听(watch send(true) 即
/// drop listener 终局——收口/监督换代的确定性收口面)。
fn spawn_serve_gen(
    listener: TcpListener,
    app: axum::Router,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::select! {
            _ = shutdown.changed() => {}
            r = axum::serve(listener, app) => {
                if let Err(e) = r {
                    tracing::warn!(%e, "预览回环服务退出(监督环将重绑)");
                }
            }
        }
    })
}

/// 新令牌(32B 随机 hex;首绑与重绑共用)。
fn fresh_token() -> String {
    use rand::RngCore as _;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

impl PreviewServer {
    /// 起服务:bind 127.0.0.1:0(OS 随机口),失败换口重试
    /// ([`BIND_ATTEMPTS`] 次);token 每实例一枚;registry 的常驻清扫
    /// 任务随启挂起(句柄归本结构,收口 abort)。`disk` = T5 落盘缓存
    /// (/jpeg 转码结果面;None = 未接线);`state` = daemon state 弱引用
    /// (/thumb + T15 视频装配面);`video` = 视频装配表。serve 由监督环
    /// 持有(重绑材料 registry/disk/state/video 一并入环)。
    pub(crate) async fn start(
        registry: Arc<HandleRegistry>,
        disk: Option<Arc<super::cache::PreviewDiskCache>>,
        state: Option<std::sync::Weak<crate::state::DaemonState>>,
        video: super::rendition::VideoSessions,
    ) -> io::Result<Arc<PreviewServer>> {
        // 先 bind 后装配路由 state:playlist 段 URI 绝对化要 port(T21 fix
        // round 1 Critical #1;bind 重试语义不变)
        let mut last_err: Option<io::Error> = None;
        let mut listener = None;
        for _ in 0..BIND_ATTEMPTS {
            match TcpListener::bind("127.0.0.1:0").await {
                Ok(l) => {
                    listener = Some(l);
                    break;
                }
                Err(e) => last_err = Some(e),
            }
        }
        let listener = listener
            .ok_or_else(|| last_err.unwrap_or_else(|| io::Error::other("回环口绑定反复失败")))?;
        let port = listener.local_addr()?.port();
        let sweeper = registry.spawn_sweeper();
        let token = fresh_token();
        let app = router(Arc::new(SrvCtx {
            registry: Arc::clone(&registry),
            token: token.clone(),
            port,
            cache: Arc::new(Readahead::new()),
            disk: disk.clone(),
            state: state.clone(),
            video: video.clone(),
        }));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let serve = spawn_serve_gen(listener, app, shutdown_rx.clone());
        let creds = Arc::new(std::sync::RwLock::new(LoopbackCreds { port, token }));
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let rebind = Arc::new(tokio::sync::Notify::new());
        let shutdown = Arc::new(shutdown_tx);
        let sup_creds = Arc::clone(&creds);
        let sup_rebind = Arc::clone(&rebind);
        let sup_closed = Arc::clone(&closed);
        let mut sup_shutdown_rx = shutdown_rx.clone();
        let supervisor = tokio::spawn(async move {
            // 当前代句柄由本环自持(重绑换代;收口经 Notify 即弃)
            let mut cur = serve;
            loop {
                // fast = 快路重绑(健康检查/看门狗/击杀触发:跳过 500ms
                // 退避,bind 微秒级当场换口);自然终局 = 慢路(退避防热环)
                let fast;
                tokio::select! {
                    _ = sup_shutdown_rx.changed() => {
                        // 收口广播:当前 serve 代自听自终(listener drop),
                        // 本环直接退(不重绑)
                        return;
                    }
                    // 快路重绑臂:abort 当前代立即重绑(r55 #3)
                    _ = sup_rebind.notified() => {
                        cur.abort();
                        fast = true;
                    }
                    res = &mut cur => {
                        let _ = res;
                        fast = false;
                    }
                }
                if sup_closed.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                tracing::warn!(fast, "预览回环 serve 终局,重绑自愈");
                loop {
                    if !fast {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        if sup_closed.load(std::sync::atomic::Ordering::SeqCst) {
                            return;
                        }
                    }
                    match TcpListener::bind("127.0.0.1:0").await {
                        Ok(l) => {
                            let port = match l.local_addr() {
                                Ok(a) => a.port(),
                                Err(_) => continue,
                            };
                            let token = fresh_token();
                            let app = router(Arc::new(SrvCtx {
                                registry: Arc::clone(&registry),
                                token: token.clone(),
                                port,
                                cache: Arc::new(Readahead::new()),
                                disk: disk.clone(),
                                state: state.clone(),
                                video: video.clone(),
                            }));
                            let serve = spawn_serve_gen(l, app, shutdown_rx.clone());
                            cur = serve;
                            *sup_creds.write().unwrap() = LoopbackCreds { port, token };
                            tracing::info!(port, "预览回环已重绑(自愈)");
                            break;
                        }
                        Err(e) => {
                            tracing::warn!(%e, "预览回环重绑失败,退避重试");
                        }
                    }
                }
            }
        });
        // r55 #3 周期看门狗:每 5s 自连当前口(本机 TCP 零负担);连不上
        // 即快路重绑——「挂而不死」形态在用户触碰前自愈(无感知面)。
        let wd_creds = Arc::clone(&creds);
        let wd_rebind = Arc::clone(&rebind);
        let wd_closed = Arc::clone(&closed);
        let watchdog = tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                if wd_closed.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                let port = wd_creds.read().unwrap().port;
                if !port_alive(port).await {
                    tracing::warn!(port, "看门狗:回环口失联,快路重绑");
                    wd_rebind.notify_one();
                }
            }
        });
        Ok(Arc::new(PreviewServer {
            creds,
            closed,
            shutdown,
            rebind,
            supervisor,
            watchdog,
            sweeper,
        }))
    }

    pub(crate) fn port(&self) -> u16 {
        self.creds.read().unwrap().port
    }

    pub(crate) fn token(&self) -> String {
        self.creds.read().unwrap().token.clone()
    }

    /// 收口(评审 Important #2;调用面 = [`PreviewGate::shutdown`] ←
    /// `Daemon::shutdown`):置收口旗(监督环见旗不再重绑)→ abort 监督
    /// 环与清扫 → abort 当前 serve(槽内)。句柄不 await(shutdown 主链
    /// 路不挂第三方任务收尾,取消在下个让渡点即生效)。
    fn abort_tasks(&self) {
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = self.shutdown.send(true);
        self.supervisor.abort();
        self.watchdog.abort();
        self.sweeper.abort();
    }

    /// 两后台任务是否都已终局(单测锚:收口断言面)。
    #[cfg(test)]
    fn tasks_finished(&self) -> bool {
        self.supervisor.is_finished() && self.sweeper.is_finished()
    }

    /// 测试面:击杀当前 serve(r54 #3 自愈钉——模拟挂起/恢复期 listener
    /// 死亡;监督环应自动重绑,`port()` 应翻新)。
    #[cfg(test)]
    fn kill_serve_for_test(&self) {
        self.rebind.notify_one();
    }

    /// r55 #3 懒健康检查:自连当前口;失联 = 快路强制重绑并等新口就位
    /// (bind 微秒级,2s 兜底)——preview_open 调用面同次返回新口。
    pub(crate) async fn ensure_healthy(&self) {
        let before = self.port();
        if port_alive(before).await {
            return;
        }
        tracing::warn!(port = before, "懒健康检查:回环口失联,快路重绑");
        self.rebind.notify_one();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            if self.port() != before {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        // 2s 未换口 = 重绑路径异常,保留旧口返回(调用面如实失败可重试)
    }
}

/// 路由装配(鉴权层裹全部 /f 路由;占位路由 501/404 在鉴权**之后**——
/// 无 token 探测不到面的存在性)。
fn router(ctx: Arc<SrvCtx>) -> Router {
    use axum::middleware;
    Router::new()
        .route("/f/{h}/raw", get(raw))
        .route("/f/{h}/thumb", get(thumb))
        .route("/f/{h}/jpeg", get(jpeg))
        .route("/f/{h}/video/index.m3u8", get(hls_index))
        .route("/f/{h}/video/seg/{seg}", get(hls_seg))
        .layer(middleware::from_fn_with_state(Arc::clone(&ctx), auth))
        .with_state(ctx)
}

/// Bearer 鉴权缝(V4-1 T20 起**双缝**,控制器裁定②):缺/错 = 401(先于
/// 路由与 handle 判定;token 全等比对——回环面无时序侧信道预算)。
/// - `Authorization: Bearer <token>`(UI fetch 面,既有);
/// - `?t=<token>` 查询参数等价(previewer bundle 内 fetch 不带头、
///   `<img>`/`<video>` 直连更无法携头——仅回环 GET 面,风险等价 Bearer;
///   token hex 串无编码歧义)。任一成立即过。
async fn auth(
    State(ctx): State<Arc<SrvCtx>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    // CORS(field 2026-11-22 修复):webview 源(Tauri http://tauri.localhost)
    // 对 127.0.0.1 回环的 fetch 是跨源请求——WebView2/WebKit 执法 CORS,
    // 无 Access-Control 头 = 所有预览 fetch 被拦(daemon 全链手动 curl 通、
    // UI 四类全败的现场差异即此;单测/CI 均 mock fetch 或原生 HTTP 客户端,
    // 真 webview 面首次实测)。preflight(OPTIONS)永不携带 token(浏览器
    // 协议规定),豁免直答——实际 GET 仍全量 token 门,安全性不变;token
    // 门在 URL/Bearer 双缝,CORS `*` 不放行任何无凭据读取。503 等错误体
    // 也携 ACAO:previewer 需跨源读 seg_retry 词面(T21 消费面)。
    if req.method() == axum::http::Method::OPTIONS {
        let mut resp = StatusCode::NO_CONTENT.into_response();
        let h = resp.headers_mut();
        h.insert("access-control-allow-origin", HeaderValue::from_static("*"));
        h.insert(
            "access-control-allow-methods",
            HeaderValue::from_static("GET, OPTIONS"),
        );
        h.insert("access-control-allow-headers", HeaderValue::from_static("*"));
        h.insert("access-control-max-age", HeaderValue::from_static("86400"));
        return resp;
    }
    let header_ok = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == format!("Bearer {}", ctx.token));
    let query_ok = req.uri().query().is_some_and(|q| {
        q.split('&').any(|kv| {
            match kv.split_once('=') {
                // 键恰为 `t` 且值全等(token hex,无 percent 编码歧义)
                Some(("t", v)) => v == ctx.token,
                _ => false,
            }
        })
    });
    if !(header_ok || query_ok) {
        let mut resp = StatusCode::UNAUTHORIZED.into_response();
        resp.headers_mut().insert(
            "access-control-allow-origin",
            HeaderValue::from_static("*"),
        );
        return resp;
    }
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert("access-control-allow-origin", HeaderValue::from_static("*"));
    // r44(用户实机「大 PDF 弹窗秒败」根修):跨源 fetch 的响应头默认只
    // 暴露 6 个安全名单头,Content-Range 不在内——pdf.ts 的
    // parseRangeTotal 读 null → 总长兜底=首块 2MB → xref(文件尾)越界
    // = 大于 2MB 的 PDF 桌面弹窗秒败(iOS 同源代理不犯)。显式暴露
    // range 消费面三头。
    h.insert(
        "access-control-expose-headers",
        HeaderValue::from_static("Content-Range, Accept-Ranges, Content-Length"),
    );
    resp
}

/// 206/200 应答装配的公共头(Accept-Ranges + 206 的 Content-Range)。
fn raw_headers(status: StatusCode, content_range: Option<String>) -> Response {
    let mut resp = status.into_response();
    resp.headers_mut().insert(
        axum::http::header::ACCEPT_RANGES,
        HeaderValue::from_static("bytes"),
    );
    if let Some(cr) = content_range {
        resp.headers_mut().insert(
            axum::http::header::CONTENT_RANGE,
            HeaderValue::from_str(&cr).expect("Content-Range 头合法"),
        );
    }
    resp
}

/// 分块产流状态(unfold;permit 随流终了归还——请求在途即占并发帽)。
struct RawStreamState {
    entry: Arc<super::handles::Entry>,
    cache: Arc<Readahead>,
    /// 剩余闭区间端点(含);start > end 即流尽。
    start: u64,
    end: u64,
    /// 首块预载(应答前装货;首块命中零二次读)。
    preloaded: Option<(u64, Arc<[u8]>)>,
    /// 块读重试余量(r44 防抖:中继瞬断自愈窗;耗尽才诚实截断)。
    block_retries: u32,
    /// 并发帽 permit(RAII 持帽面——字段无读取点,随流终了 Drop 归还)。
    #[allow(dead_code)]
    permit: tokio::sync::OwnedSemaphorePermit,
}

impl RawStreamState {
    async fn next_chunk(mut self) -> Option<(Result<axum::body::Bytes, io::Error>, Self)> {
        // 流尽 / close 即停 / 空源(size==0 全量 = 空体直出,不落块读)
        if self.start > self.end || self.entry.is_closed() || self.start >= self.entry.size {
            return None;
        }
        // 逐块刷新空闲时钟(评审 Important #1):活动流每产一块即 touch,
        // 空闲回收不拦腰截断长流;块间隙的漏判由 sweep 的持帽豁免闭合。
        let block_idx = self.start / READAHEAD_BLOCK;
        let block = match self.preloaded.take() {
            Some((idx, b)) if idx == block_idx => b,
            _ => {
                if self.entry.is_closed() {
                    return None;
                }
                // r44 防抖(用户裁定「先做防抖,网络可靠性」):块读失败 =
                // 有界重试(中继瞬断/连接抖动自愈——下次 ctrl 请求经池自动
                // 剔死连重拨;此前一断即截断应答 = 中继大视频 30-40s 随机
                // 「加载失败」根因)。退避 1s;余量耗尽才诚实截断。
                let source = Arc::clone(&self.entry.source);
                let mut attempt = 0u32;
                loop {
                    match self
                        .cache
                        .block(self.entry.id, block_idx, Arc::clone(&source), self.entry.size)
                        .await
                    {
                        Ok(b) => break b,
                        Err(e) => {
                            if self.entry.is_closed() || attempt >= self.block_retries {
                                tracing::warn!(%e, retries = attempt, "预览流中块读失败(重试耗尽),截断收尾");
                                return None;
                            }
                            attempt += 1;
                            tracing::warn!(%e, retries_left = self.block_retries + 1 - attempt, "预览流中块读失败,防抖重试");
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        }
                    }
                }
            }
        };
        let boff = self.start - block_idx * READAHEAD_BLOCK;
        if boff >= block.len() as u64 {
            return None; // 源短读尽(实际尺寸小于快照,诚实收尾)
        }
        // 分块上限:块内切 ≤256KiB 分量(流式背压粒度,与 READ_CHUNK 同责)
        let take =
            ((block.len() as u64 - boff).min(self.end - self.start + 1) as usize).min(256 * 1024);
        let chunk = axum::body::Bytes::copy_from_slice(&block[boff as usize..boff as usize + take]);
        self.start += take as u64;
        self.entry.touch();
        Some((Ok(chunk), self))
    }
}

/// `GET /f/{h}/raw`:Range → 206/200/416;handle 未知/过期 = 410;首块
/// 预载失败 = 503;流体逐块检 closed 即停(close 后资源即停)。
async fn raw(
    State(ctx): State<Arc<SrvCtx>>,
    Path(h): Path<String>,
    headers: HeaderMap,
) -> Response {
    // 鉴权已过缝;先验 handle(410),再验 Range(400/416),最后占并发帽
    let Some(entry) = ctx.registry.resolve(&h) else {
        return (
            StatusCode::GONE,
            "句柄不存在或已过期,请重新申请(preview_open)",
        )
            .into_response();
    };
    let size = entry.size;
    let range = parse_range(headers.get(axum::http::header::RANGE), size);
    let (status, start, end) = match range {
        Err(RangeErr::Unsatisfiable) => {
            let mut resp = raw_headers(StatusCode::RANGE_NOT_SATISFIABLE, None);
            resp.headers_mut().insert(
                axum::http::header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{size}")).expect("静态头合法"),
            );
            return resp;
        }
        Err(RangeErr::Malformed) => {
            return (StatusCode::BAD_REQUEST, "Range header 畸形").into_response();
        }
        Ok(RangeOutcome::Full) => (StatusCode::OK, 0u64, size.saturating_sub(1)),
        Ok(RangeOutcome::Part { start, end }) => (StatusCode::PARTIAL_CONTENT, start, end),
    };
    // 并发帽(第 5 路在此排队;permit 随流终了归还)
    let Ok(permit) = Arc::clone(&entry.range_permits).acquire_owned().await else {
        return (StatusCode::SERVICE_UNAVAILABLE, "并发帽信号量已闭").into_response();
    };
    // 首块应答前装货:首读失败 = 503(源不可达面,客户端回退/重试);
    // 空区间(start>end,含 size==0 全量)零块直出。
    let mut preloaded = None;
    if size > 0 && start <= end {
        let block_idx = start / READAHEAD_BLOCK;
        match ctx
            .cache
            .block(entry.id, block_idx, Arc::clone(&entry.source), size)
            .await
        {
            Ok(b) => preloaded = Some((block_idx, b)),
            Err(e) => {
                tracing::warn!(%e, "预览首块装货失败");
                return (StatusCode::SERVICE_UNAVAILABLE, format!("源读取失败: {e}"))
                    .into_response();
            }
        }
    }
    let state = RawStreamState {
        entry,
        cache: Arc::clone(&ctx.cache),
        start,
        end,
        preloaded,
        block_retries: RAW_BLOCK_RETRY_MAX,
        permit,
    };
    let content_range =
        (status == StatusCode::PARTIAL_CONTENT).then(|| format!("bytes {start}-{end}/{size}"));
    let mut resp = raw_headers(status, content_range);
    let body = Body::from_stream(futures_util::stream::unfold(
        state,
        RawStreamState::next_chunk,
    ));
    *resp.body_mut() = body;
    resp
}

/// `/f/{h}/thumb?w=` 默认宽(spec §2 路由示例 256;缺省/非法值同归此)。
const THUMB_DEFAULT_WIDTH: u32 = 256;

/// `GET /f/{h}/thumb?w=`:缩略图透传(V4-1 T7)。语义(brief 钉死):
/// 响应 = jpeg bytes 或**空 200**(失败/无后端恒 200 空体,客户端回退
/// 类型图标;不回 4xx/5xx——缩略图缺位只是降级面);handle 未知/过期 =
/// 410(鉴权先行于一切,既有语义不变)。路由:本机 handle(device 为
/// self 别名/本机 id/未登录 "local")= 引擎面直驱(`thumb-tmp` 缓存 +
/// 平台后端);远端 handle = ctrl `thumb` 转发到源端(失败=空字节——
/// 对端离线/旧对端不识新变体都属「缩略图暂不可得」,Review Focus 5 互
/// 通降级面,不上抛)。`w` 钳制到引擎面上界(巨 w 内存炸弹面)。
async fn thumb(
    State(ctx): State<Arc<SrvCtx>>,
    Path(h): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(entry) = ctx.registry.resolve(&h) else {
        return (
            StatusCode::GONE,
            "句柄不存在或已过期,请重新申请(preview_open)",
        )
            .into_response();
    };
    entry.touch();
    let w = q
        .get("w")
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|&w| w > 0)
        .unwrap_or(THUMB_DEFAULT_WIDTH);
    let bytes = match ctx.state.as_ref().and_then(std::sync::Weak::upgrade) {
        // state 未注入(测试缺省面)/已收尾:恒空字节回退
        None => Vec::new(),
        Some(state) => {
            let (device, path, mtime) = entry.thumb_identity();
            let size = entry.size;
            if device == "local" || crate::ctrl::is_self(&state, &device) {
                // 本机臂:**路由化单点**(r32 修——此前直调 thumb() 缓存+
                // 平台后端,桥臂不可达 = 手机自览视频/PDF 缩略图恒空;与
                // ctrl 应答面(thumb_reply)同点收口,桥根/照片/fs 三臂
                // 同路由;桌面行为等价——无桥根时 routed 退化为 thumb())
                super::thumb::thumb_routed(&state, &path, mtime, size, w).await
            } else {
                // 远端臂:ctrl thumb 转发(失败=空字节回退,见函数注)。
                // r44(用户实机「缩略图每次都现取」根修):**查看端缓存**——
                // 收到即入 preview_thumb_cache(键=与本地同口径
                // hash(path|mtime|size|w));二次进视口零网络零 ctrl 往返
                // (源端缓存只省源端生成,网络往返躲不掉的是此面)。空字节
                // 不入缓存(T7 语义)。
                let key = super::thumb::cache_key(&path, mtime, size, w);
                let cache = state.preview_thumb_cache();
                if let Some(hit) = cache.load(&key) {
                    hit // 缓存命中:零网络零 ctrl 往返
                } else {
                    let req = mtdss_proto::ctrl::ThumbRequest {
                        path,
                        mtime,
                        size: size as i64,
                        width: w,
                    };
                    // r50 #2:远端 thumb 单发抖动有界重试(1s 退避恰一次)。
                    // 仅传输面错误进重试——空 jpeg 是「源端无缩略图」的成
                    // 功应答,不烧重试;仍败 = 空字节回退图标(降级面不变)。
                    let mut attempt = 0u8;
                    loop {
                        match state.ctrl().preview_thumb(&state, &device, req.clone()).await {
                            Ok(r) => {
                                if !r.jpeg.is_empty() {
                                    cache.store(&key, &r.jpeg);
                                }
                                break r.jpeg;
                            }
                            Err(e) if attempt == 0 => {
                                attempt += 1;
                                tracing::debug!(
                                    device = %device, err = e.rpc().1,
                                    "远端缩略图首败(抖动?),1s 后重试恰一次"
                                );
                                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            }
                            Err(e) => {
                                tracing::debug!(device = %device, err = e.rpc().1, "远端缩略图不可得,空字节回退");
                                break Vec::new();
                            }
                        }
                    }
                }
            }
        }
    };
    let mut resp = (StatusCode::OK, bytes).into_response();
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("image/jpeg"),
    );
    resp
}

/// `GET /f/{h}/jpeg` 应答装配(200 + Content-Type: image/jpeg)。
#[cfg(all(windows, feature = "heic"))]
fn jpeg_response(bytes: Vec<u8>) -> Response {
    let mut resp = (StatusCode::OK, bytes).into_response();
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("image/jpeg"),
    );
    resp
}

/// `GET /f/{h}/jpeg`:HEIC 源解码转 JPEG(V4-1 T6,仅 Windows + heic
/// feature)。语义(brief 钉死):非 HEIC → 404(客户端回退 raw);解码
/// 失败 → 422 不 panic;libheif 运行库缺席(部署面)→ 503;结果进 T5
/// 落盘缓存(handle 为键,`path_for` 命中即零重解码;T5 的在预览豁免使
/// 缓存件活到 handle 关闭后的下一轮淘汰,重复浏览免重转码)。整取面有
/// 帽([`JPEG_SOURCE_CAP_BYTES`],评审 Important #1:超帽源读前即拒,
/// 远控巨型文件不进 RAM)。
#[cfg(all(windows, feature = "heic"))]
async fn jpeg(State(ctx): State<Arc<SrvCtx>>, Path(h): Path<String>) -> Response {
    let Some(entry) = ctx.registry.resolve(&h) else {
        return (
            StatusCode::GONE,
            "句柄不存在或已过期,请重新申请(preview_open)",
        )
            .into_response();
    };
    entry.touch();
    // 整取帽先拒(评审 Important #1):按 size 快照判定,巨型源零读源
    // 即 422——4GB 级远控文件不进 RAM。快照定界(开源时核对),无 TOCTOU
    // 面;超帽恒 422(哪怕实为非 HEIC——读前无从嗅探,文案向客户端说明
    // 回退 raw 即可)。
    let size = entry.size;
    if size > JPEG_SOURCE_CAP_BYTES {
        tracing::warn!(
            size,
            cap = JPEG_SOURCE_CAP_BYTES,
            "JPEG 转码面源超整取帽,拒"
        );
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("源超转码整取帽({size} > {JPEG_SOURCE_CAP_BYTES} 字节),不整取(客户端回退 raw)"),
        )
            .into_response();
    }
    // 缓存命中:转码结果已在 preview-tmp(handle 键),零重解码直出。
    // 缓存件被外力消失 = warn 后落转码路径(store 即恢复)。
    if let Some(disk) = &ctx.disk {
        if let Some(path) = disk.path_for(&h) {
            match tokio::fs::read(&path).await {
                Ok(bytes) => return jpeg_response(bytes),
                Err(e) => {
                    tracing::warn!(%e, path = %path.display(), "JPEG 缓存件读取失败,重新转码")
                }
            }
        }
    }
    // 全量读源(spawn_blocking:read_at 是阻塞调用,fs/远端同款纪律;
    // size 取开源快照定界——变更面在 open 处已由声明核对拦截)。
    let source = Arc::clone(&entry.source);
    let read = tokio::task::spawn_blocking(move || {
        let mut buf = Vec::new();
        let mut off = 0u64;
        while off < size {
            let mut chunk = vec![0u8; ((size - off) as usize).min(256 * 1024)];
            let n = source.read_at(off, &mut chunk)?;
            if n == 0 {
                break; // EOF(源短于快照,诚实呈现)
            }
            buf.extend_from_slice(&chunk[..n]);
            off += n as u64;
        }
        Ok::<_, std::io::Error>(buf)
    })
    .await;
    let bytes = match read {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => {
            tracing::warn!(%e, "JPEG 转码面读源失败");
            return (StatusCode::SERVICE_UNAVAILABLE, format!("源读取失败: {e}")).into_response();
        }
        Err(join) => {
            tracing::warn!(%join, "JPEG 转码面读源任务收尾失败");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("读源任务收尾失败: {join}"),
            )
                .into_response();
        }
    };
    // 非 HEIC → 404(嗅探先于解码:HEIC 外源零接触 libheif)
    if !super::heic::sniff_heic(&bytes) {
        return (
            StatusCode::NOT_FOUND,
            "非 HEIC 源,无 JPEG 转码(客户端回退 raw)",
        )
            .into_response();
    }
    // 解码转码(spawn_blocking:libheif 解码是 CPU 密集阻塞面)
    let decoded =
        tokio::task::spawn_blocking(move || super::heic::decode_heic_to_jpeg(&bytes)).await;
    let jpeg = match decoded {
        Ok(Ok(j)) => j,
        Ok(Err(super::heic::DecodeError::Unavailable)) => {
            tracing::warn!("libheif 运行库不可用,HEIC 转码面降级");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "转码器不可用(未部署 libheif.dll)",
            )
                .into_response();
        }
        Ok(Err(super::heic::DecodeError::Bad(msg))) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("HEIC 解码失败: {msg}"),
            )
                .into_response();
        }
        Err(join) => {
            tracing::warn!(%join, "JPEG 转码任务收尾失败");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("转码任务收尾失败: {join}"),
            )
                .into_response();
        }
    };
    // 结果入 T5 落盘缓存(best-effort:失败仅留痕,本次照常直出)
    if let Some(disk) = &ctx.disk {
        if let Err(e) = disk.store(&h, &mut std::io::Cursor::new(&jpeg)) {
            tracing::warn!(%e, "JPEG 转码结果缓存失败(本次直出)");
        }
    }
    jpeg_response(jpeg)
}

/// 非 Windows / 未启用 heic:404 占位(T6 仅 Windows 面;客户端回退 raw)。
#[cfg(not(all(windows, feature = "heic")))]
async fn jpeg(State(_ctx): State<Arc<SrvCtx>>, Path(_h): Path<String>) -> Response {
    (
        StatusCode::NOT_FOUND,
        "转码面未实装(本平台未启用 HEIC 转码)",
    )
        .into_response()
}

/// HLS playlist MIME(brief 钉死;常量锚,改值须过测试)。
const PLAYLIST_MIME: &str = "application/vnd.apple.mpegurl";

/// 段 Content-Type 头装配(T15-M2 一行修):open 应答透传的 mime 原样
/// 入库;远端毒值(非法 header 字节/控制符/非 ASCII)不得崩请求面——
/// 旧形 `expect` 毒值即 panic(远端可控输入打崩查看端单请求面),修 =
/// fallback fMP4 稳定 MIME(`video/iso.segment`),单请求降级不毒崩。
fn segment_mime_header(mime: &str) -> HeaderValue {
    HeaderValue::from_str(mime)
        .unwrap_or_else(|_| HeaderValue::from_static(super::rendition::SEGMENT_MIME))
}

/// 视频装配取用/惰性建(V4-1 T15):表内既有即用;缺 = 按 handle 身份
/// 装配 fetch(本机 device = 进程内直驱分发缝;远端 = ctrl 池)→ attach
/// → 入表。竞窗:双首请求各自 attach 末位入表(先至者由源端 60s 空闲
/// 回收兜底,幂等面)。
enum VideoSessionErr {
    /// 源端无转码后端 → 回退原流+海报(404 面)。
    Unsupported,
    /// 其余失败(503 面,稳定码+文案)。
    Failed(String),
}

async fn video_session(
    ctx: &SrvCtx,
    state: &Arc<crate::state::DaemonState>,
    entry: &Arc<super::handles::Entry>,
    handle: &str,
) -> Result<Arc<super::rendition::ViewerRendition>, VideoSessionErr> {
    if let Some(v) = ctx.video.lock().unwrap().get(handle).cloned() {
        return Ok(v);
    }
    let (device, path, mtime) = entry.thumb_identity();
    let fetch = if device == "local" || crate::ctrl::is_self(state, &device) {
        // 本机臂:进程内直驱分发缝(守卫/路由/限流与远端 wire 同缝)
        super::rendition::RenditionFetch::local(state)
    } else {
        // 远端臂:ctrl 池 rendition 三 op
        super::rendition::RenditionFetch::remote(state, &device)
    };
    let vs = super::rendition::ViewerRendition::attach(fetch, &path, mtime, entry.size)
        .await
        .map_err(|e| match e {
            super::rendition::AttachError::Unsupported => VideoSessionErr::Unsupported,
            super::rendition::AttachError::Failed(c, m) => {
                VideoSessionErr::Failed(format!("{c}: {m}"))
            }
        })?;
    let vs = Arc::new(vs);
    ctx.video
        .lock()
        .unwrap()
        .insert(handle.to_string(), Arc::clone(&vs));
    Ok(vs)
}

/// `GET /f/{h}/video/index.m3u8`(V4-1 T15):动态 playlist——handle 解析
/// (410/触视频装配 detach)→ 装配惰性 attach(unsupported = 404 回退
/// 面)→ 前缀顺序推进(推进窗内拉新段;失败面 M5 = 503 勿吞为段尽)→
/// 已缓存段列出(未拉段不列,段序无洞;EVENT 形无 ENDLIST)。
async fn hls_index(State(ctx): State<Arc<SrvCtx>>, Path(h): Path<String>) -> Response {
    let Some(entry) = ctx.registry.resolve(&h) else {
        ctx_detach_video(&ctx, &h);
        return (
            StatusCode::GONE,
            "句柄不存在或已过期,请重新申请(preview_open)",
        )
            .into_response();
    };
    entry.touch();
    let Some(state) = ctx.state.as_ref().and_then(std::sync::Weak::upgrade) else {
        return (StatusCode::SERVICE_UNAVAILABLE, "预览引擎不可达").into_response();
    };
    let vs = match video_session(&ctx, &state, &entry, &h).await {
        Ok(v) => v,
        Err(VideoSessionErr::Unsupported) => {
            return (
                StatusCode::NOT_FOUND,
                "视频转码不可用(源端无转码后端),回退原流+海报",
            )
                .into_response();
        }
        Err(VideoSessionErr::Failed(e)) => {
            return (StatusCode::SERVICE_UNAVAILABLE, format!("转码装配失败: {e}"))
                .into_response();
        }
    };
    let cache = state.preview_cache();
    match vs.pull_segments(&cache, &h).await {
        Err((code, msg)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("拉段失败({code}): {msg}"),
        )
            .into_response(),
        Ok(_) => {
            // 段 URI 绝对化(T21 fix round 1 Critical #1):相对 URI 会被
            // hls.js 按 RFC 3986 解析丢查询串 → 段请求 401;此处单点拼
            // 绝对前缀 + 查询令牌,MAP/EXTINF 全携(全客户端受益)。
            let seg_prefix = format!("http://127.0.0.1:{}/f/{h}/video/seg", ctx.port);
            let mut resp = (StatusCode::OK, vs.playlist_m3u8(&seg_prefix, &ctx.token)).into_response();
            resp.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static(PLAYLIST_MIME),
            );
            resp
        }
    }
}

/// `GET /f/{h}/video/seg/{n}.m4s`(V4-1 T15):命中 = 缓存件读盘直出
/// (M3 消费时过存在性);未拉 = 跳段直取目标段(乱序直达源端,有界
/// 等待 10s → 503;窗过半仍未达 = 远跳重开换道,T15b:cancel+携
/// start_at 重开+余窗续等,装配内部收口;越界 = 404 止步词;源离线 =
/// 503 + 稳定词;超界 503 携「重试」稳定词 + Retry-After 头)。段名仅收
/// `{n}.m4s`(playlist 所发之形;其余 = 400)。
async fn hls_seg(
    State(ctx): State<Arc<SrvCtx>>,
    Path((h, seg)): Path<(String, String)>,
) -> Response {
    let Some(entry) = ctx.registry.resolve(&h) else {
        ctx_detach_video(&ctx, &h);
        return (
            StatusCode::GONE,
            "句柄不存在或已过期,请重新申请(preview_open)",
        )
            .into_response();
    };
    entry.touch();
    let Some(index) = seg
        .strip_suffix(".m4s")
        .and_then(|s| s.parse::<u32>().ok())
    else {
        return (StatusCode::BAD_REQUEST, "段名畸形(仅 seg/{n}.m4s)").into_response();
    };
    let Some(state) = ctx.state.as_ref().and_then(std::sync::Weak::upgrade) else {
        return (StatusCode::SERVICE_UNAVAILABLE, "预览引擎不可达").into_response();
    };
    let vs = match video_session(&ctx, &state, &entry, &h).await {
        Ok(v) => v,
        Err(VideoSessionErr::Unsupported) => {
            return (
                StatusCode::NOT_FOUND,
                "视频转码不可用(源端无转码后端),回退原流+海报",
            )
                .into_response();
        }
        Err(VideoSessionErr::Failed(e)) => {
            return (StatusCode::SERVICE_UNAVAILABLE, format!("转码装配失败: {e}"))
                .into_response();
        }
    };
    let cache = state.preview_cache();
    match vs.ensure_segment(&cache, &h, index).await {
        Ok(bytes) => {
            let mut resp = (StatusCode::OK, bytes).into_response();
            resp.headers_mut()
                .insert(axum::http::header::CONTENT_TYPE, segment_mime_header(&vs.mime()));
            resp
        }
        Err(super::rendition::SegFail::Timeout) => {
            // 503 携「重试」语义(T15b;T21 消费):体含稳定词 `seg_retry`
            // + 头携 Retry-After(秒)——同段稍后重试可达(远跳重开已在
            // 等待路径自动发生,本面 = 重开后余窗仍尽)。
            let mut resp = (
                StatusCode::SERVICE_UNAVAILABLE,
                format!(
                    "转码段等待超界(有界等待 {}s,段未就绪,{},稍后重试)",
                    super::rendition::RENDITION_SEG_WAIT.as_secs(),
                    super::rendition::SEG_RETRY_WORD,
                ),
            )
                .into_response();
            resp.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                HeaderValue::from_str(&super::rendition::SEG_RETRY_AFTER_SECS.to_string())
                    .expect("Retry-After 数字秒值合法"),
            );
            resp
        }
        Err(super::rendition::SegFail::Range) => (
            StatusCode::NOT_FOUND,
            format!("段越界(止步): seg/{index}"),
        )
            .into_response(),
        Err(super::rendition::SegFail::Failed(c, m)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("段获取失败({c}): {m}"),
        )
            .into_response(),
    }
}

/// 视频路由 410 面的装配摘除(preview_close 同语义:后台 detach,源端
/// 会话随 handle 释放;gate 表与路由表同份)。
fn ctx_detach_video(ctx: &SrvCtx, handle: &str) {
    let Some(v) = ctx.video.lock().unwrap().remove(handle) else {
        return;
    };
    tokio::spawn(async move { v.detach().await });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::handles::CloseOutcome;
    use crate::preview::source::FsFileSource;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::time::Duration;

    /// 图样(ctrl_e2e 同款,逐字节可复算)。
    fn pattern(i: u64) -> u8 {
        (i.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as u8
    }

    /// 读计数源(窗口命中断言面):记 read_at 调用次数。
    struct CountingSource {
        file: FsFileSource,
        reads: AtomicUsize,
    }
    impl CountingSource {
        fn open(path: &std::path::Path) -> Arc<Self> {
            Arc::new(Self {
                file: FsFileSource::open(path).unwrap(),
                reads: AtomicUsize::new(0),
            })
        }
    }
    impl FileSource for CountingSource {
        fn size(&self) -> u64 {
            self.file.size()
        }
        fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
            self.reads.fetch_add(1, AtomicOrdering::SeqCst);
            self.file.read_at(off, buf)
        }
    }

    /// 慢速假源(close 即停断言面):每次 read_at 隔 20ms 后整段填图样
    /// (预读块逐块装货即逐块 20ms,给 close 留出落在流中的窗口)。
    struct SlowSource {
        size: u64,
    }
    impl FileSource for SlowSource {
        fn size(&self) -> u64 {
            self.size
        }
        fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
            std::thread::sleep(Duration::from_millis(20));
            let want = (self.size.saturating_sub(off) as usize).min(buf.len());
            for (i, b) in buf[..want].iter_mut().enumerate() {
                *b = pattern(off + i as u64);
            }
            Ok(want)
        }
    }

    /// 恒败源(首读失败 → 503 面)。
    struct FailingSource(u64);
    impl FileSource for FailingSource {
        fn size(&self) -> u64 {
            self.0
        }
        fn read_at(&self, _off: u64, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("源读失败"))
        }
    }

    /// r54 #3/r55 #3 自愈钉:击杀当前 serve(模拟挂起/恢复期 listener
    /// 死亡)→ 监督环重绑——端口翻新、新口可 TCP 连接;**快路面**:
    /// ensure_healthy 在口死时同次调用内换新口(无 500ms 退避)。
    #[tokio::test]
    async fn serve_supervisor_rebinds_after_kill() {
        let (srv, _reg) = test_server().await;
        // 懒健康检查健康面:活口直返(零副作用)
        let t0 = std::time::Instant::now();
        srv.ensure_healthy().await;
        assert!(t0.elapsed() < std::time::Duration::from_millis(300), "健康口应快返");
        let port0 = srv.port();
        let token0 = srv.token();
        srv.kill_serve_for_test();
        // 快路:懒检查当场驱动重绑(bind 微秒级;2s 兜底内应换口)
        let t1 = std::time::Instant::now();
        srv.ensure_healthy().await;
        let port1 = srv.port();
        assert_ne!(port0, port1, "重绑后端口应翻新");
        assert_ne!(srv.token(), token0, "重绑后令牌应换新");
        assert!(
            t1.elapsed() < std::time::Duration::from_secs(2),
            "快路重绑应在兜底窗内完成"
        );
        assert!(
            std::net::TcpStream::connect(("127.0.0.1", port1)).is_ok(),
            "新口应可 TCP 连接(监听在场)"
        );
        srv.abort_tasks();
    }

    /// 测试服务(独立 registry + 127.0.0.1 随机口;无落盘缓存接线)。
    async fn test_server() -> (Arc<PreviewServer>, Arc<HandleRegistry>) {
        let reg = Arc::new(HandleRegistry::new());
        let srv = PreviewServer::start(reg.clone(), None, None, Default::default())
            .await
            .expect("起服务");
        (srv, reg)
    }

    /// T7 测试服务(带 daemon state 弱引用接线的 /thumb 面;TempDir 保活
    /// 面返回——state 的 home 即 thumb-tmp 缓存目录)。
    async fn thumb_test_server() -> (
        Arc<PreviewServer>,
        Arc<HandleRegistry>,
        std::sync::Arc<crate::state::DaemonState>,
        tempfile::TempDir,
    ) {
        let home = tempfile::tempdir().expect("临时 home");
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
        let reg = Arc::new(HandleRegistry::new());
        let srv = PreviewServer::start(
            reg.clone(),
            None,
            Some(std::sync::Arc::downgrade(&state)),
            Default::default(),
        )
        .await
        .expect("起服务");
        (srv, reg, state, home)
    }

    /// T6 测试服务(带 T5 落盘缓存接线的 /jpeg 面;TempDir 保活面返回)。
    #[cfg(all(windows, feature = "heic"))]
    async fn jpeg_test_server() -> (
        Arc<PreviewServer>,
        Arc<HandleRegistry>,
        std::sync::Arc<crate::preview::cache::PreviewDiskCache>,
        tempfile::TempDir,
    ) {
        let reg = Arc::new(HandleRegistry::new());
        let home = tempfile::tempdir().expect("临时 home");
        let disk = Arc::new(crate::preview::cache::PreviewDiskCache::new(
            home.path(),
            Arc::clone(&reg),
        ));
        let srv = PreviewServer::start(reg.clone(), Some(Arc::clone(&disk)), None, Default::default())
            .await
            .expect("起服务");
        (srv, reg, disk, home)
    }

    fn bearer(srv: &PreviewServer) -> String {
        format!("Bearer {}", srv.token())
    }

    /// Range 解析矩阵(纯函数):合法单段三形(闭区间收口/开放尾/suffix)、
    /// 无 header 全量、off≥size 与 suffix 0 = 416、畸形五形 = 400。
    #[test]
    fn range_parse_matrix() {
        let size = 1000u64;
        let hv = |s: &str| HeaderValue::from_str(s).unwrap();
        // 无 header → 全量
        assert_eq!(parse_range(None, size), Ok(RangeOutcome::Full));
        // 合法单段
        assert_eq!(
            parse_range(Some(&hv("bytes=0-99")), size),
            Ok(RangeOutcome::Part { start: 0, end: 99 })
        );
        assert_eq!(
            parse_range(Some(&hv("bytes=500-")), size),
            Ok(RangeOutcome::Part {
                start: 500,
                end: 999
            }),
            "开放尾收口到源末"
        );
        assert_eq!(
            parse_range(Some(&hv("bytes=-100")), size),
            Ok(RangeOutcome::Part {
                start: 900,
                end: 999
            }),
            "suffix 100 = 末 100 字节"
        );
        assert_eq!(
            parse_range(Some(&hv("bytes=-5000")), size),
            Ok(RangeOutcome::Part { start: 0, end: 999 }),
            "suffix 超长 = 全量段(206)"
        );
        assert_eq!(
            parse_range(Some(&hv("bytes=999-9999")), size),
            Ok(RangeOutcome::Part {
                start: 999,
                end: 999
            }),
            "end 越界收口到源末"
        );
        // 不可满足 → 416
        assert_eq!(
            parse_range(Some(&hv("bytes=1000-")), size),
            Err(RangeErr::Unsatisfiable)
        );
        assert_eq!(
            parse_range(Some(&hv("bytes=2000-2050")), size),
            Err(RangeErr::Unsatisfiable)
        );
        assert_eq!(
            parse_range(Some(&hv("bytes=-0")), size),
            Err(RangeErr::Unsatisfiable),
            "len=0"
        );
        // 畸形 → 400
        assert_eq!(
            parse_range(Some(&hv("bytes=5-4")), size),
            Err(RangeErr::Malformed)
        );
        assert_eq!(
            parse_range(Some(&hv("bytes=abc-")), size),
            Err(RangeErr::Malformed)
        );
        assert_eq!(
            parse_range(Some(&hv("bytes=1e2-")), size),
            Err(RangeErr::Malformed)
        );
        assert_eq!(
            parse_range(Some(&hv("bytes=0-1,3-4")), size),
            Err(RangeErr::Malformed),
            "多段不支持 = 畸形"
        );
        assert_eq!(
            parse_range(Some(&hv("items=0-1")), size),
            Err(RangeErr::Malformed)
        );
        assert_eq!(
            parse_range(Some(&hv("bytes=")), size),
            Err(RangeErr::Malformed)
        );
        assert_eq!(
            parse_range(Some(&hv("bytes=-")), size),
            Err(RangeErr::Malformed)
        );
        assert_eq!(
            parse_range(Some(&hv("bytes=99999999999999999999999-")), size),
            Err(RangeErr::Malformed),
            "溢出 = 畸形"
        );
    }

    /// 路由矩阵(brief 钉死):鉴权(缺/错=401)→ raw 全量/三段 206 →
    /// 416/400 矩阵 → 未知 handle 410 → 占位 501/404 → close 后 410。
    #[tokio::test]
    async fn raw_route_matrix() {
        // 1000 字节图样文件
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.bin");
        let data: Vec<u8> = (0..1000u64).map(pattern).collect();
        std::fs::write(&path, &data).unwrap();

        let (srv, reg) = test_server().await;
        let base = format!("http://127.0.0.1:{}", srv.port());
        let h = reg
            .open(
                "local",
                path.to_string_lossy().as_ref(),
                Some(1),
                Some(1000),
                CountingSource::open(&path),
            )
            .unwrap_or_else(|e| panic!("开句柄: {e:?}"));
        let raw_url = format!("{base}/f/{h}/raw");
        let client = reqwest::Client::new();

        // 鉴权:缺 token / 错 token = 401(先于 handle 判定——坏 handle 同 401)
        let r = client.get(&raw_url).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "缺 token = 401");
        let r = client
            .get(&raw_url)
            .header("Authorization", "Bearer 错的")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "错 token = 401");
        let r = client
            .get(format!("{base}/f/不存在/raw"))
            .header("Authorization", "Bearer 错的")
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::UNAUTHORIZED,
            "未知 handle 错 token = 401(鉴权先行于 410)"
        );

        let tok = reqwest::header::HeaderValue::from_str(&bearer(&srv)).unwrap();

        // 无 Range → 200 全量 + Accept-Ranges
        let r = client
            .get(&raw_url)
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            r.headers()
                .get("Accept-Ranges")
                .map(|v| v.to_str().unwrap()),
            Some("bytes")
        );
        assert_eq!(r.bytes().await.unwrap(), &data[..], "全量字节一致");

        // 三段 206
        let r = client
            .get(&raw_url)
            .header("Authorization", &tok)
            .header("Range", "bytes=0-99")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            r.headers()
                .get("Content-Range")
                .map(|v| v.to_str().unwrap()),
            Some("bytes 0-99/1000")
        );
        assert_eq!(r.bytes().await.unwrap(), &data[..100], "head 段一致");

        let r = client
            .get(&raw_url)
            .header("Authorization", &tok)
            .header("Range", "bytes=500-")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(r.bytes().await.unwrap(), &data[500..], "开放尾段一致");

        let r = client
            .get(&raw_url)
            .header("Authorization", &tok)
            .header("Range", "bytes=-100")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(r.bytes().await.unwrap(), &data[900..], "suffix 段一致");

        // 416 矩阵(off ≥ size / suffix 0;携 bytes */size)
        for spec in ["bytes=1000-", "bytes=2000-2050", "bytes=-0"] {
            let r = client
                .get(&raw_url)
                .header("Authorization", &tok)
                .header("Range", spec)
                .send()
                .await
                .unwrap();
            assert_eq!(
                r.status(),
                StatusCode::RANGE_NOT_SATISFIABLE,
                "{spec} → 416"
            );
            assert_eq!(
                r.headers()
                    .get("Content-Range")
                    .map(|v| v.to_str().unwrap()),
                Some("bytes */1000"),
                "{spec} 携不可满足指示"
            );
        }

        // 400 矩阵(畸形)
        for spec in [
            "bytes=5-4",
            "bytes=abc-",
            "bytes=0-1,3-4",
            "items=0-1",
            "bytes=",
        ] {
            let r = client
                .get(&raw_url)
                .header("Authorization", &tok)
                .header("Range", spec)
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{spec} → 400");
        }

        // 未知 handle → 410
        let r = client
            .get(format!("{base}/f/deadbeef/raw"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::GONE, "未知 handle = 410");

        // 占位路由:jpeg 404 / video 404(鉴权后);thumb 已 T7 实装
        // (非图源空字节 200,本机臂),见 thumb_route 专项用例。
        let r = client
            .get(format!("{base}/f/{h}/jpeg"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "jpeg 占位 404(T6)");
        // 视频路由(T15 实装):本测试服务无 state 注入(测试缺省面)=
        // 503 预览引擎不可达(handle 已解析、鉴权已过——路由矩阵面只钉
        // 缺省臂;真实视频面归 video_route_* 专项用例)。
        let r = client
            .get(format!("{base}/f/{h}/video/index.m3u8"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "video playlist(无 state 缺省面)= 503"
        );
        let r = client
            .get(format!("{base}/f/{h}/video/seg/3.m4s"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "video seg(无 state 缺省面)= 503"
        );

        // 空源(size==0):全量 200 空体;bytes=0-0 = 416(off ≥ size)
        let h0 = reg
            .open(
                "local",
                "empty.bin",
                Some(1),
                Some(0),
                Arc::new(FailingSource(0)),
            )
            .unwrap_or_else(|e| panic!("开空源句柄: {e:?}"));
        let r = client
            .get(format!("{base}/f/{h0}/raw"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "空源全量 200");
        assert!(r.bytes().await.unwrap().is_empty(), "空源零字节");
        let r = client
            .get(format!("{base}/f/{h0}/raw"))
            .header("Authorization", &tok)
            .header("Range", "bytes=0-0")
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::RANGE_NOT_SATISFIABLE,
            "空源任何段 = 416"
        );

        // close 后即 410(未知 handle 同面)
        assert_eq!(reg.close(&h), CloseOutcome::Removed);
        let r = client
            .get(&raw_url)
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::GONE, "close 后 raw = 410");
    }

    /// 查询令牌等价 Bearer(V4-1 T20 控制器裁定②):回环 GET 面增
    /// `?t=<token>` 缝——previewer bundle 内 fetch 不带头、`<img>`/`<video>`
    /// 直连更无法携 Authorization,凭据改走查询串。正确 t= → 200;错 t/
    /// 空值/无凭据恒 401(鉴权先行语义不变,与 handle 判定先后同既有);
    /// 头与查询串任一成立即过。Bearer 头矩阵仍见 raw_route_matrix。
    #[tokio::test]
    async fn query_token_auth_matrix() {
        let (srv, reg) = test_server().await;
        let base = format!("http://127.0.0.1:{}", srv.port());
        let client = reqwest::Client::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.bin");
        std::fs::write(&path, b"hello").unwrap();
        let h = reg
            .open(
                "local",
                path.to_string_lossy().as_ref(),
                Some(1),
                Some(5),
                CountingSource::open(&path),
            )
            .unwrap();

        // 带 t= 正确 → 200(无 Authorization 头),字节一致
        let r = client
            .get(format!("{base}/f/{h}/raw?t={}", srv.token()))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "查询令牌正确 = 200");
        // CORS(field 2026-11-22):webview 跨源面——成功应答必须携 ACAO
        assert_eq!(
            r.headers()
                .get("access-control-allow-origin")
                .and_then(|v| v.to_str().ok()),
            Some("*"),
            "成功应答携 ACAO(webview 跨源 fetch 执法)"
        );
        assert_eq!(r.bytes().await.unwrap(), &b"hello"[..], "查询令牌面字节一致");

        // CORS preflight:OPTIONS 永不携 token(浏览器协议)→ 豁免直答
        // 204 + 预检头;实际 GET 仍全量 token 门。
        let r = client
            .request(axum::http::Method::OPTIONS, format!("{base}/f/{h}/raw"))
            .header("origin", "http://tauri.localhost")
            .header("access-control-request-method", "GET")
            .header("access-control-request-headers", "range")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NO_CONTENT, "preflight 无 token 直答 204");
        assert_eq!(
            r.headers()
                .get("access-control-allow-origin")
                .and_then(|v| v.to_str().ok()),
            Some("*"),
            "preflight 携 ACAO"
        );
        assert_eq!(
            r.headers()
                .get("access-control-allow-headers")
                .and_then(|v| v.to_str().ok()),
            Some("*"),
            "preflight 允许头 *(Range 等非 safelist 头)"
        );

        // 401 面也携 ACAO(previewer 跨源读错误体面)
        let r = client
            .get(format!("{base}/f/{h}/raw"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            r.headers()
                .get("access-control-allow-origin")
                .and_then(|v| v.to_str().ok()),
            Some("*"),
            "401 应答携 ACAO"
        );

        // 与既有查询参数共存(thumb 的 w=)
        let r = client
            .get(format!("{base}/f/{h}/thumb?w=64&t={}", srv.token()))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "查询令牌与 w= 共存");

        // 错 t → 401(先于 handle 判定)
        let r = client
            .get(format!("{base}/f/无此句柄/raw?t=错的"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "错查询令牌 = 401");

        // t= 空值 / 其余参数 ≠ 凭据
        let r = client
            .get(format!("{base}/f/{h}/raw?t="))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "空 t = 401");
        let r = client
            .get(format!("{base}/f/{h}/raw?w=64"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "无凭据 = 401");

        // 头错但 t 对 = 200(双缝任一成立即过)
        let r = client
            .get(format!("{base}/f/{h}/raw?t={}", srv.token()))
            .header("Authorization", "Bearer 错的")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "头错 t 对 = 200(任一凭据成立)");
    }

    /// 预读窗口命中:同一 2MiB 块内连续两 range,第二发免重读源(源
    /// read_at 次数不增);跨块第三发补读一块。LRU 出清:小帽注入下第三
    /// 块挤掉第一块(块数受帽收敛)。
    #[tokio::test]
    async fn readahead_window_hits_consecutive_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.bin");
        let len = 3 * READAHEAD_BLOCK; // 恰三块
        let data: Vec<u8> = (0..len).map(pattern).collect();
        std::fs::write(&path, &data).unwrap();

        let cache = Readahead::with_cap(2 * READAHEAD_BLOCK as usize); // 帽 2 块
        let src = CountingSource::open(&path);

        // 同块两取:块 0 的 [0,64KiB) 与 [64KiB,128KiB)
        let b0 = cache.block(1, 0, src.clone(), len).await.unwrap();
        assert_eq!(&b0[..100], &data[..100], "块 0 头部字节一致");
        assert_eq!(b0.len(), READAHEAD_BLOCK as usize, "整块装货");
        let reads_after_first = src.reads.load(AtomicOrdering::SeqCst);
        assert!(reads_after_first >= 1, "首块触发源读");

        let b0_again = cache.block(1, 0, src.clone(), len).await.unwrap();
        assert_eq!(
            src.reads.load(AtomicOrdering::SeqCst),
            reads_after_first,
            "同块再取 = 窗口命中,零源读"
        );
        assert!(Arc::ptr_eq(&b0, &b0_again), "命中同一缓存块");

        // 出清:帽 2 块,装第三块 → 最早块(0)出清
        let _b1 = cache.block(1, 1, src.clone(), len).await.unwrap();
        assert_eq!(cache.len(), 2, "帽内两块");
        let _b2 = cache.block(1, 2, src.clone(), len).await.unwrap();
        assert_eq!(cache.len(), 2, "第三块入缓存挤掉最旧一块");
        drop(b0_again);
        assert_eq!(Arc::strong_count(&b0), 1, "被挤块仅剩测试引用");

        // 出清后重取块 0 = 重新读源(次序刷新)
        let reads_before = src.reads.load(AtomicOrdering::SeqCst);
        let _b0_re = cache.block(1, 0, src.clone(), len).await.unwrap();
        assert!(
            src.reads.load(AtomicOrdering::SeqCst) > reads_before,
            "出清块重取 = 重新读源"
        );
    }

    /// 尾部短块:块界落在源末(非整块),块长 = 余量;EOF 不越界。
    #[tokio::test]
    async fn readahead_tail_block_short() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tail.bin");
        let len = READAHEAD_BLOCK + 100; // 第二块仅 100 字节
        let data: Vec<u8> = (0..len).map(pattern).collect();
        std::fs::write(&path, &data).unwrap();
        let cache = Readahead::new();
        let src = CountingSource::open(&path);
        let b1 = cache.block(1, 1, src, len).await.unwrap();
        assert_eq!(b1.len(), 100, "尾块恰给余量");
        assert_eq!(&b1[..], &data[READAHEAD_BLOCK as usize..], "尾块字节一致");
    }

    /// close 后资源即停(Review Focus):慢源在途流,关 handle → 新请求
    /// 410,在途流体在下一块界即停(收到的字节 << 请求区间)。客户端读
    /// 首块后停拉,close 落地,生产端在下一块界检 closed 旗标收流——
    /// 排干余量后总量必须远小于请求区间(块装货 20ms/块 = 生产端步调,
    /// 不可能事先跑完 16MiB)。
    #[tokio::test]
    async fn close_stops_inflight_reads() {
        let (srv, reg) = test_server().await;
        let base = format!("http://127.0.0.1:{}", srv.port());
        const WANT: u64 = 16 * 1024 * 1024;
        let h = reg
            .open(
                "local",
                "slow.bin",
                Some(1),
                Some(WANT),
                Arc::new(SlowSource { size: WANT }),
            )
            .unwrap();
        let tok = reqwest::header::HeaderValue::from_str(&bearer(&srv)).unwrap();
        let client = reqwest::Client::new();

        // 在途流:全量 16MiB;读首个分块(≤256KiB)即停拉 + close(在途)
        let mut stream = client
            .get(format!("{base}/f/{h}/raw"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(stream.status(), StatusCode::OK);
        let mut got = 0usize;
        while let Some(chunk) = stream.chunk().await.unwrap() {
            got += chunk.len();
            if got >= 64 * 1024 {
                break; // 首分块到手即关(在途)
            }
        }
        assert_eq!(reg.close(&h), CloseOutcome::Removed, "close 即关");
        // 排干剩余:closed 旗标在块界即停,总量必须远小于请求区间
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            match tokio::time::timeout_at(deadline, stream.chunk()).await {
                Err(_) => panic!("流体未在限期收口(close 即停失守)"),
                Ok(Err(e)) => panic!("排干读败: {e}"),
                Ok(Ok(None)) => break,
                Ok(Ok(Some(c))) => got += c.len(),
            }
        }
        assert!(
            (got as u64) < WANT,
            "close 后在途流必须提前收口(收 {got} < 请求 {WANT})"
        );
        // 关后新请求 = 410
        let r = client
            .get(format!("{base}/f/{h}/raw"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::GONE, "关后新请求 410");
    }

    /// 首读失败 = 503(源不可达面:远端离线/源 io 失败在首块装货处显形,
    /// 客户端回退/重试)。
    #[tokio::test]
    async fn first_block_failure_is_503() {
        let (srv, reg) = test_server().await;
        let base = format!("http://127.0.0.1:{}", srv.port());
        let h = reg
            .open(
                "local",
                "x.bin",
                Some(1),
                Some(100),
                Arc::new(FailingSource(100)),
            )
            .unwrap();
        let tok = reqwest::header::HeaderValue::from_str(&bearer(&srv)).unwrap();
        let r = reqwest::Client::new()
            .get(format!("{base}/f/{h}/raw"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "首块装货失败 = 503"
        );
    }

    /// Gate 惰性单例:两次 server() 同实例同口(第二启零 bind)。
    #[tokio::test]
    async fn gate_lazy_singleton() {
        let gate = PreviewGate::new();
        let s1 = gate.server().await.expect("首启");
        let s2 = gate.server().await.expect("复用");
        assert!(Arc::ptr_eq(&s1, &s2), "同一实例");
        // registry 面不开服务也可用(close 幂等)
        assert_eq!(
            gate.registry().close("无此句柄"),
            CloseOutcome::Unknown,
            "registry 直驱面"
        );
    }

    /// 收口面(评审 Important #2):gate.shutdown 后 serve+清扫两任务终局
    /// (不留泄漏任务),监听撤除——端口连接必败;未启门的 shutdown 纯
    /// no-op。daemon 级接线由 ctrl_e2e 的本机面用例收尾断言钉(真走
    /// Daemon::shutdown)。
    #[tokio::test]
    async fn gate_shutdown_stops_listener_and_sweeper() {
        let gate = PreviewGate::new();
        let srv = gate.server().await.expect("起服务");
        let port = srv.port();
        // 活监听:连接即达
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("收口前监听在");
        gate.shutdown();
        // 两后台任务终局(abort 在下个让渡点生效)
        let mut ticks = 0;
        while !srv.tasks_finished() {
            tokio::task::yield_now().await;
            ticks += 1;
            assert!(ticks < 200, "收口后 serve/清扫任务未终局");
        }
        // 端口死:监听已撤,连接必败(拒绝或超时皆算败,不许成功)
        let conn = tokio::time::timeout(
            Duration::from_millis(2000),
            tokio::net::TcpStream::connect(("127.0.0.1", port)),
        )
        .await;
        assert!(
            !matches!(&conn, Ok(Ok(_))),
            "收口后端口必须死(监听已撤): {conn:?}"
        );
        // 未启门的 shutdown 纯 no-op(无任务可 abort,不得 panic)
        PreviewGate::new().shutdown();
    }

    /// 在途流豁免空闲回收(评审 Important #1):慢源流时长(800ms)超过
    /// idle ttl(300ms)——拖过界后 sweep,在途 A(持帽+逐块 touch)必须
    /// 存活至终局全量字节一致;闲置对照 B 照常回收;流毕闲置后 A 亦照常
    /// 回收。老实现(无豁免/无逐块 touch)在 sweep 时点 A 的 idle 已 >ttl,
    /// 流被拦腰截断且与短读不可区分——本用例即其回归面。
    #[tokio::test]
    async fn inflight_stream_survives_idle_reclaim() {
        let ttl = Duration::from_millis(300);
        let reg = Arc::new(HandleRegistry::with_params(ttl));
        let srv = PreviewServer::start(reg.clone(), None, None, Default::default())
            .await
            .expect("起服务");
        let base = format!("http://127.0.0.1:{}", srv.port());
        // B 对照:小件真文件,闲置即回收
        let dir = tempfile::tempdir().unwrap();
        let b_path = dir.path().join("idle.bin");
        std::fs::write(&b_path, b"1234567890").unwrap();
        let h_b = reg
            .open(
                "local",
                b_path.to_string_lossy().as_ref(),
                Some(1),
                Some(10),
                CountingSource::open(&b_path),
            )
            .unwrap();
        // B 先闲置过界(此刻注册表只有 B,回收面不受 A 影响)
        tokio::time::sleep(ttl * 2).await;

        // A:慢源 16MiB(8 块 × 100ms ≈ 800ms 流时长,远超 ttl)
        const WANT: u64 = 16 * 1024 * 1024;
        let h_a = reg
            .open(
                "local",
                "slow.bin",
                Some(1),
                Some(WANT),
                Arc::new(SlowSource { size: WANT }),
            )
            .unwrap();
        let tok = reqwest::header::HeaderValue::from_str(&bearer(&srv)).unwrap();
        let client = reqwest::Client::new();
        let mut stream = client
            .get(format!("{base}/f/{h_a}/raw"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(stream.status(), StatusCode::OK);
        // 首分块在手 = 请求在途(permit 已持,resolve 已 touch)
        let mut got = 0usize;
        while let Some(chunk) = stream.chunk().await.unwrap() {
            got += chunk.len();
            if got >= 64 * 1024 {
                break;
            }
        }
        // 拖过 idle 界后 sweep:老实现此刻 A 的 idle > ttl 即被截断
        tokio::time::sleep(ttl + Duration::from_millis(150)).await;
        reg.sweep();
        assert!(
            reg.resolve(&h_a).is_some(),
            "在途流不得被空闲回收(持帽豁免+逐块 touch)"
        );
        assert!(reg.resolve(&h_b).is_none(), "闲置对照条目照常回收");

        // A 流到终局:全量 16MiB 字节一致(流未被打断)
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            match tokio::time::timeout_at(deadline, stream.chunk()).await {
                Err(_) => panic!("A 流未在限期收口"),
                Ok(Err(e)) => panic!("A 流读败: {e}"),
                Ok(Ok(None)) => break,
                Ok(Ok(Some(c))) => got += c.len(),
            }
        }
        assert_eq!(got as u64, WANT, "在途流存活至终局,全量字节完整");

        // 流毕(permit 已还)+闲置超界 → 照常回收(豁免不放大成永生)
        std::thread::sleep(ttl * 2);
        reg.sweep();
        assert!(reg.resolve(&h_a).is_none(), "流毕闲置后照常回收");
    }

    // ---------- /thumb 缩略图透传面(V4-1 T7) ----------

    /// T7 /thumb 路由矩阵:未知 handle 410 → 本机非图源空字节 200
    /// (Content-Type image/jpeg;空=失败面恒 200 语义)→ 远端 handle
    /// 设备离线空字节 200(快错,Review Focus 5 降级)→ w 缺省照常。
    #[tokio::test]
    async fn thumb_route_passthrough_matrix() {
        let (srv, reg, _state, home) = thumb_test_server().await;
        let base = format!("http://127.0.0.1:{}", srv.port());
        let tok = reqwest::header::HeaderValue::from_str(&bearer(&srv)).unwrap();
        let client = reqwest::Client::new();

        // 未知 handle = 410(鉴权先行既有语义)
        let r = client
            .get(format!("{base}/f/无此句柄/thumb?w=64"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::GONE, "未知 handle thumb = 410");

        // 本机非图源(.txt):200 空体(失败恒 200,客户端回退类型图标)
        let path = home.path().join("note.txt");
        std::fs::write(&path, b"plain text, no thumbnail backend").unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        let h = reg
            .open(
                "local",
                path.to_string_lossy().as_ref(),
                Some(1),
                Some(size),
                CountingSource::open(&path),
            )
            .unwrap();
        let r = client
            .get(format!("{base}/f/{h}/thumb?w=64"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "非图源恒 200");
        assert_eq!(
            r.headers().get("Content-Type").map(|v| v.to_str().unwrap()),
            Some("image/jpeg"),
            "thumb 面的 Content-Type"
        );
        assert!(r.bytes().await.unwrap().is_empty(), "非图源 = 空体");

        // 远端 handle(device = 未注册设备):离线快错 → 空字节 200(不
        // 悬等打洞;Review Focus 5 互通降级同面)
        let h_remote = reg
            .open(
                "别家的设备",
                "/远端/路径.jpg",
                Some(1),
                Some(100),
                Arc::new(FailingSource(100)),
            )
            .unwrap();
        let t0 = std::time::Instant::now();
        let r = client
            .get(format!("{base}/f/{h_remote}/thumb?w=64"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "远端失败恒 200");
        assert!(r.bytes().await.unwrap().is_empty(), "远端不可达 = 空体");
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(5),
            "离线快错不悬等: {:?}",
            t0.elapsed()
        );

        // w 缺省(=256)照常走面(非图源空体,只钉路由语义)
        let r = client
            .get(format!("{base}/f/{h}/thumb"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "w 缺省照常");
    }

    /// T7 /thumb 本机图片臂(Windows 真后端):JPEG handle → 200 非空
    /// jpeg 且 image crate 可解、尺寸 ≤ w。缓存命中面归 thumb.rs 单测
    /// (GEN_CALLS 增量),此处只钉 HTTP 透传形状。
    #[cfg(windows)]
    #[tokio::test]
    async fn thumb_route_local_jpeg_renders() {
        let _serial = crate::preview::thumb::TEST_SERIAL.lock().await;
        let (srv, reg, _state, home) = thumb_test_server().await;
        let base = format!("http://127.0.0.1:{}", srv.port());
        let tok = reqwest::header::HeaderValue::from_str(&bearer(&srv)).unwrap();
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(96, 64, |x, y| {
            image::Rgb([(x * 3 % 256) as u8, (y * 4 % 256) as u8, 90])
        }));
        let path = home.path().join("pic.jpg");
        image::codecs::jpeg::JpegEncoder::new_with_quality(
            std::fs::File::create(&path).unwrap(),
            90,
        )
        .encode_image(&img)
        .unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        let h = reg
            .open(
                "local",
                path.to_string_lossy().as_ref(),
                Some(1),
                Some(size),
                CountingSource::open(&path),
            )
            .unwrap();
        let r = reqwest::Client::new()
            .get(format!("{base}/f/{h}/thumb?w=64"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            r.headers().get("Content-Type").map(|v| v.to_str().unwrap()),
            Some("image/jpeg")
        );
        let body = r.bytes().await.unwrap();
        let decoded = image::load_from_memory(&body).expect("JPEG 可解");
        assert!(
            decoded.width() <= 64 && decoded.height() <= 64,
            "等比入槽 ≤w: {}x{}",
            decoded.width(),
            decoded.height()
        );
    }

    // ---------- /jpeg HEIC 转码面(V4-1 T6;仅 Windows + heic feature) ----------

    /// HEIC 夹具(现编,brief「往返自足,免外部二进制」):libheif 自带
    /// HEVC 编码器写一枚 w×h HEIC,返回路径与实际字节数。测试 DLL 须带
    /// x265(vcpkg 默认含;见 heic 模块注)。
    #[cfg(all(windows, feature = "heic"))]
    fn fixture_heic(dir: &std::path::Path, w: u32, h: u32) -> (std::path::PathBuf, u64) {
        let path = dir.join(format!("photo_{w}x{h}.heic"));
        let len = crate::preview::heic::fixture::encode_test_heic(w, h, &path);
        (path, len)
    }

    /// 夹具 HEIC → /jpeg 200(brief 钉死):image crate 解码校验尺寸与
    /// Content-Type。全程持解码串行锁(缓存命中用例的计数增量不失真)。
    #[cfg(all(windows, feature = "heic"))]
    #[tokio::test]
    async fn jpeg_heic_renders_200_with_verified_dims() {
        let _guard = crate::preview::heic::TEST_SERIAL.lock().await;
        let (srv, reg, _disk, _home) = jpeg_test_server().await;
        let base = format!("http://127.0.0.1:{}", srv.port());
        let dir = tempfile::tempdir().unwrap();
        let (path, size) = fixture_heic(dir.path(), 64, 48);
        let h = reg
            .open(
                "local",
                path.to_string_lossy().as_ref(),
                Some(1),
                Some(size),
                CountingSource::open(&path),
            )
            .unwrap_or_else(|e| panic!("开夹具句柄: {e:?}"));
        let tok = reqwest::header::HeaderValue::from_str(&bearer(&srv)).unwrap();
        let r = reqwest::Client::new()
            .get(format!("{base}/f/{h}/jpeg"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "夹具 HEIC → 200");
        assert_eq!(
            r.headers().get("Content-Type").map(|v| v.to_str().unwrap()),
            Some("image/jpeg"),
            "JPEG 面的 Content-Type"
        );
        let body = r.bytes().await.unwrap();
        let img = image::load_from_memory(&body).expect("image crate 可解");
        assert_eq!((img.width(), img.height()), (64, 48), "解码尺寸一致");
    }

    /// 损坏件 → 422 不 panic(brief 钉死):截半保留 ftyp 头(嗅探过、
    /// mdat 残缺),解码失败映 422,服务面照常(后续请求不残)。
    #[cfg(all(windows, feature = "heic"))]
    #[tokio::test]
    async fn jpeg_corrupt_heic_is_422_no_panic() {
        let _guard = crate::preview::heic::TEST_SERIAL.lock().await;
        let (srv, reg, _disk, _home) = jpeg_test_server().await;
        let base = format!("http://127.0.0.1:{}", srv.port());
        let dir = tempfile::tempdir().unwrap();
        let (path, size) = fixture_heic(dir.path(), 48, 32);
        let raw = std::fs::read(&path).unwrap();
        let cut = raw.len() / 2;
        assert!(cut > 12 && cut < size as usize, "截断保住 ftyp 头且非空");
        let cut_path = dir.path().join("cut.heic");
        std::fs::write(&cut_path, &raw[..cut]).unwrap();
        let h = reg
            .open(
                "local",
                cut_path.to_string_lossy().as_ref(),
                Some(1),
                Some(cut as u64),
                CountingSource::open(&cut_path),
            )
            .unwrap();
        let tok = reqwest::header::HeaderValue::from_str(&bearer(&srv)).unwrap();
        let client = reqwest::Client::new();
        let r = client
            .get(format!("{base}/f/{h}/jpeg"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "损坏件 422(不 panic 不 5xx)"
        );
        // 服务面无残:同 handle 再请求同面(仍 422,不挂死)
        let r = client
            .get(format!("{base}/f/{h}/jpeg"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY, "重复请求同面");
    }

    /// 非 HEIC → 404(brief 钉死,客户端回退 raw):嗅探先于解码,PNG 头
    /// 与纯文本字节都不进 libheif。
    #[cfg(all(windows, feature = "heic"))]
    #[tokio::test]
    async fn jpeg_non_heic_is_404() {
        let _guard = crate::preview::heic::TEST_SERIAL.lock().await;
        let (srv, reg, _disk, _home) = jpeg_test_server().await;
        let base = format!("http://127.0.0.1:{}", srv.port());
        let dir = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            ("pic.png", &b"\x89PNG\r\n\x1a\njunkjunkjunkjunk"[..]),
            ("note.txt", b"hello, this is not an image at all"),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            let h = reg
                .open(
                    "local",
                    path.to_string_lossy().as_ref(),
                    Some(1),
                    Some(bytes.len() as u64),
                    CountingSource::open(&path),
                )
                .unwrap();
            let tok = reqwest::header::HeaderValue::from_str(&bearer(&srv)).unwrap();
            let r = reqwest::Client::new()
                .get(format!("{base}/f/{h}/jpeg"))
                .header("Authorization", &tok)
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::NOT_FOUND, "{name} → 404(回退 raw)");
        }
    }

    /// 缓存命中零重解码(brief 钉死):首取解码一次并入 T5 缓存;二取
    /// 命中直出——解码计数不增、字节一致、缓存件在账。增量断言(非绝对
    /// 值)容他进程内用例的并行解码。
    #[cfg(all(windows, feature = "heic"))]
    #[tokio::test]
    async fn jpeg_cache_hit_skips_redecode() {
        use std::sync::atomic::Ordering as AtomicOrdering;
        let _guard = crate::preview::heic::TEST_SERIAL.lock().await;
        let (srv, reg, disk, _home) = jpeg_test_server().await;
        let base = format!("http://127.0.0.1:{}", srv.port());
        let dir = tempfile::tempdir().unwrap();
        let (path, size) = fixture_heic(dir.path(), 40, 30);
        let h = reg
            .open(
                "local",
                path.to_string_lossy().as_ref(),
                Some(1),
                Some(size),
                CountingSource::open(&path),
            )
            .unwrap();
        let tok = reqwest::header::HeaderValue::from_str(&bearer(&srv)).unwrap();
        let url = format!("{base}/f/{h}/jpeg");
        let client = reqwest::Client::new();
        let decodes = || crate::preview::heic::DECODE_CALLS.load(AtomicOrdering::SeqCst);

        // 增量断言(串行锁在手,窗内唯一解码面 = 本用例的请求)
        let c0 = decodes();
        let r1 = client
            .get(&url)
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r1.status(), StatusCode::OK);
        let body1 = r1.bytes().await.unwrap();
        let c1 = decodes();
        assert_eq!(c1, c0 + 1, "首取恰解码一次(增量)");
        assert!(disk.path_for(&h).is_some(), "转码结果已入 T5 缓存");

        let r2 = client
            .get(&url)
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r2.status(), StatusCode::OK);
        let body2 = r2.bytes().await.unwrap();
        assert_eq!(body2, body1, "缓存命中字节一致");
        assert_eq!(decodes(), c1, "二取零重解码(计数不增)");
    }

    /// 超帽假源(评审 Important #1 覆盖面):size = 帽+1,无实体数据;
    /// read_at 计数——被触达即证「整取前未拒」的回归面。
    #[cfg(all(windows, feature = "heic"))]
    struct HugeSource {
        reads: AtomicUsize,
    }
    #[cfg(all(windows, feature = "heic"))]
    impl FileSource for HugeSource {
        fn size(&self) -> u64 {
            JPEG_SOURCE_CAP_BYTES + 1
        }
        fn read_at(&self, _off: u64, _buf: &mut [u8]) -> io::Result<usize> {
            self.reads.fetch_add(1, AtomicOrdering::SeqCst);
            Err(io::Error::other("超帽源不得被读"))
        }
    }

    /// 超帽源 → 422 且零读源(评审 Important #1):整取前按 size 快照拒
    /// ,远端把 /jpeg 指向巨型文件时不进 RAM;重复请求同面(不残)。对照
    /// 臂:帽内夹具 HEIC 照常 200。200 臂会解码,持串行锁护缓存命中用例
    /// 的计数增量。
    #[cfg(all(windows, feature = "heic"))]
    #[tokio::test]
    async fn jpeg_oversized_source_is_422_without_reading() {
        let _guard = crate::preview::heic::TEST_SERIAL.lock().await;
        let (srv, reg, _disk, _home) = jpeg_test_server().await;
        let base = format!("http://127.0.0.1:{}", srv.port());
        let tok = reqwest::header::HeaderValue::from_str(&bearer(&srv)).unwrap();
        let client = reqwest::Client::new();

        let huge = Arc::new(HugeSource {
            reads: AtomicUsize::new(0),
        });
        let h = reg
            .open(
                "local",
                "huge.heic",
                Some(1),
                Some(huge.size()),
                huge.clone() as Arc<dyn FileSource>,
            )
            .unwrap();
        let url = format!("{base}/f/{h}/jpeg");
        let r = client
            .get(&url)
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY, "超帽 422");
        assert_eq!(
            huge.reads.load(AtomicOrdering::SeqCst),
            0,
            "超帽源零读源(整取前拒)"
        );
        let r = client
            .get(&url)
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY, "重复请求同面");

        // 对照:帽内小 HEIC 照常 200(帽不误伤正常量级)
        let dir = tempfile::tempdir().unwrap();
        let (path, size) = fixture_heic(dir.path(), 32, 24);
        let h2 = reg
            .open(
                "local",
                path.to_string_lossy().as_ref(),
                Some(1),
                Some(size),
                CountingSource::open(&path),
            )
            .unwrap();
        let r = client
            .get(format!("{base}/f/{h2}/jpeg"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "帽内小件照常转码");
    }

    // ---------- /f/{h}/video/* 转码面(V4-1 T15) ----------

    /// T15 测试服务(gate 全接线:registry = gate.registry、state 注入、
    /// 视频装配表 = gate 共享份;转码工厂由调用方安装;TempDir 保活面
    /// 返回——state 的 home 即 T5 段缓存目录)。
    async fn video_test_server() -> (
        Arc<PreviewGate>,
        Arc<PreviewServer>,
        Arc<HandleRegistry>,
        Arc<crate::state::DaemonState>,
        tempfile::TempDir,
    ) {
        let home = tempfile::tempdir().expect("临时 home");
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
        let gate = Arc::new(PreviewGate::new());
        gate.set_state(std::sync::Arc::downgrade(&state));
        let srv = PreviewServer::start(
            gate.registry(),
            None,
            Some(std::sync::Arc::downgrade(&state)),
            gate.video_sessions(),
        )
        .await
        .expect("起服务");
        (Arc::clone(&gate), srv, gate.registry(), state, home)
    }

    /// 垫料视频件 + 句柄(独立 tempdir 保活面随返回;4096 零字节,转码
    /// 面不触其内容,源只过手)。
    fn video_file_and_handle(
        reg: &Arc<HandleRegistry>,
    ) -> (tempfile::TempDir, String, u64, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("movie.bin");
        std::fs::write(&path, vec![0u8; 4096]).unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        let h = reg
            .open(
                "local",
                path.to_string_lossy().as_ref(),
                Some(1),
                Some(size),
                CountingSource::open(&path),
            )
            .unwrap();
        (dir, path.to_string_lossy().into_owned(), size, h)
    }

    use crate::preview::rendition::{FakeTranscoder, PseudoTranscoder};

    /// T15 e2e 相①(brief:playlist 形状):鉴权先行 → 未知 handle 410 →
    /// 首刷 playlist(**首段即列**/段序无洞/未产段不列/无 ENDLIST,
    /// MIME application/vnd.apple.mpegurl)→ init/内容段字节逐字节 +
    /// MIME video/iso.segment → 越界 404 止步词 → 段名畸形 400。
    #[tokio::test]
    async fn video_route_playlist_shape_and_segment_bytes() {
        let (_gate, srv, reg, state, _home) = video_test_server().await;
        let (factory, _made) =
            crate::preview::rendition::test_factory(FakeTranscoder::standard);
        state.rendition_sessions().install_factory(factory);
        let base = format!("http://127.0.0.1:{}", srv.port());
        let tok = reqwest::header::HeaderValue::from_str(&bearer(&srv)).unwrap();
        let client = reqwest::Client::new();
        let (_dir, _path, _size, h) = video_file_and_handle(&reg);

        // 鉴权:缺 token = 401(先于 handle 判定)
        let r = client
            .get(format!("{base}/f/{h}/video/index.m3u8"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "缺 token = 401");

        // 未知 handle = 410
        let r = client
            .get(format!("{base}/f/无此句柄/video/index.m3u8"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::GONE, "未知 handle video = 410");

        // playlist 形状(首段即列/段序无洞/未产段不列)
        let r = client
            .get(format!("{base}/f/{h}/video/index.m3u8"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            r.headers().get("Content-Type").map(|v| v.to_str().unwrap()),
            Some("application/vnd.apple.mpegurl"),
            "playlist MIME"
        );
        let pl = r.text().await.unwrap();
        assert!(pl.starts_with("#EXTM3U\n"), "M3U8 头: {pl}");
        assert!(pl.contains("#EXT-X-VERSION:7"), "版本 7: {pl}");
        assert!(pl.contains("#EXT-X-PLAYLIST-TYPE:EVENT"), "EVENT 动态形");
        assert!(pl.contains("#EXT-X-TARGETDURATION:4"), "4000ms → 4s");
        // 段 URI 绝对+携 ?t= 令牌(T21 fix round 1 Critical #1:相对 URI
        // 经 hls.js 解析丢查询串 → 段请求 401)
        assert!(
            pl.contains(&format!(
                "#EXT-X-MAP:URI=\"{base}/f/{h}/video/seg/0.m4s?t={}\"",
                srv.token()
            )),
            "fMP4 MAP 面(绝对+令牌): {pl}"
        );
        assert!(
            pl.contains(&format!(
                "#EXTINF:4.000,\n{base}/f/{h}/video/seg/1.m4s?t={}",
                srv.token()
            )),
            "首段即列(绝对+令牌): {pl}"
        );
        assert!(pl.contains("seg/2.m4s?t="), "段序无洞(前缀全列,段行同携令牌): {pl}");
        assert!(!pl.contains("seg/3"), "未产段不列: {pl}");
        // field 修:伪转码器段尽(越界止步)= 源端完毕 → playlist 封
        // ENDLIST(时长锁定全长 + 播放端结束态)
        assert!(pl.contains("#EXT-X-ENDLIST"), "段尽封 ENDLIST: {pl}");

        // init 段(0)与内容段(1)字节逐字节 + 段 MIME
        for (i, want) in [
            (0u32, PseudoTranscoder::init_bytes()),
            (1, PseudoTranscoder::segment_bytes(1)),
        ] {
            let r = client
                .get(format!("{base}/f/{h}/video/seg/{i}.m4s"))
                .header("Authorization", &tok)
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::OK, "段 {i} 200");
            assert_eq!(
                r.headers().get("Content-Type").map(|v| v.to_str().unwrap()),
                Some("video/iso.segment"),
                "段 {i} MIME"
            );
            assert_eq!(r.bytes().await.unwrap(), &want[..], "段 {i} 字节一致");
        }

        // 越界 = 404 止步词(segment_range 面;非等待面)
        let r = client
            .get(format!("{base}/f/{h}/video/seg/3.m4s"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "越界 404 止步");
        let body = r.text().await.unwrap();
        assert!(body.contains("越界"), "止步词面: {body}");

        // 段名畸形 = 400(仅收 playlist 所发之形 {n}.m4s)
        for bad in ["abc.m4s", "3", "3.mp4", "-1.m4s"] {
            let r = client
                .get(format!("{base}/f/{h}/video/seg/{bad}"))
                .header("Authorization", &tok)
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{bad} → 400");
        }
    }

    /// T15 e2e 相②(brief:seek 请求缺段→源收到跳段请求,断言桥面):
    /// 未经 playlist 前缀推进,直取段 2(拖动面)——源端调用序记录面
    /// 证词 = **乱序直达 [2]**;随后 playlist 刷新:前缀顺序推进对跳段
    /// 已拉段缓存短路(M3),段序无洞补齐。
    #[tokio::test]
    async fn video_route_seek_skip_reaches_source_out_of_order() {
        let (_gate, srv, reg, state, _home) = video_test_server().await;
        let (factory, made) =
            crate::preview::rendition::test_factory(FakeTranscoder::standard);
        state.rendition_sessions().install_factory(factory);
        let base = format!("http://127.0.0.1:{}", srv.port());
        let tok = reqwest::header::HeaderValue::from_str(&bearer(&srv)).unwrap();
        let client = reqwest::Client::new();
        let (_dir, _path, _size, h) = video_file_and_handle(&reg);

        // 跳段:直取段 2(源端收到乱序请求)
        let r = client
            .get(format!("{base}/f/{h}/video/seg/2.m4s"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "跳段直取成功");
        assert_eq!(
            r.bytes().await.unwrap(),
            &PseudoTranscoder::segment_bytes(2)[..],
            "跳段字节一致"
        );
        assert_eq!(
            made.lock().unwrap()[0].calls(),
            [2],
            "源端收到乱序跳段请求(桥面记录)"
        );

        // playlist 刷新:前缀推进(2 缓存短路),段序无洞
        let r = client
            .get(format!("{base}/f/{h}/video/index.m3u8"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let pl = r.text().await.unwrap();
        assert!(pl.contains("seg/1.m4s?t=") && pl.contains("seg/2.m4s?t="), "前缀补齐(段行携令牌): {pl}");
        assert_eq!(
            made.lock().unwrap()[0].calls(),
            [2, 0, 1, 3],
            "前缀推进对跳段已拉段零重拉(M3 短路面);段尽探测一发即止"
        );
    }

    /// T15 e2e 相③(brief:源离线→503)+ T12-M5 路由面:恒读败源 seg =
    /// 503 携稳定词;init 后中途读败 playlist = 503(勿吞为段尽出半截
    /// playlist——旧形 200 空列表即红)。
    #[tokio::test]
    async fn video_route_source_offline_is_503() {
        let (_gate, srv, reg, state, _home) = video_test_server().await;
        let (factory, _made) =
            crate::preview::rendition::test_factory(FakeTranscoder::content_failing);
        state.rendition_sessions().install_factory(factory);
        let base = format!("http://127.0.0.1:{}", srv.port());
        let tok = reqwest::header::HeaderValue::from_str(&bearer(&srv)).unwrap();
        let client = reqwest::Client::new();
        let (_dir, _path, _size, h) = video_file_and_handle(&reg);

        // seg 路由:读败 → 503 + source_offline 稳定词
        let r = client
            .get(format!("{base}/f/{h}/video/seg/1.m4s"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE, "源离线 503");
        let body = r.text().await.unwrap();
        assert!(body.contains("source_offline"), "503 携稳定词: {body}");

        // playlist 路由:init ok、内容段中途失败 = 503(M5 勿吞)
        let r = client
            .get(format!("{base}/f/{h}/video/index.m3u8"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "拉段失败勿吞为段尽(503 可重试面)"
        );
    }

    /// T15 e2e 相④(brief:10s 界,注入时钟):恒未就绪源——seg 路由有
    /// 界等待烧满 10s = 503;tokio 暂停钟注入(拨钟即烧窗,零真实等待),
    /// tokio Instant 断言整窗烧满(路由全环 tokio 计时)。T15b:503 携
    /// 「重试」语义(体含稳定词 seg_retry + 头携 Retry-After);窗过半
    /// 恰一次远跳重开(start_at = 目标段×段长),总界不放宽。
    #[tokio::test(start_paused = true)]
    async fn video_route_wait_budget_times_out_503() {
        let (_gate, srv, reg, state, _home) = video_test_server().await;
        let (factory, _made, starts) =
            crate::preview::rendition::test_factory_start_at(|_| FakeTranscoder::pending());
        state.rendition_sessions().install_factory(factory);
        let base = format!("http://127.0.0.1:{}", srv.port());
        let tok = reqwest::header::HeaderValue::from_str(&bearer(&srv)).unwrap();
        let client = reqwest::Client::new();
        let (_dir, _path, _size, h) = video_file_and_handle(&reg);

        let t0 = tokio::time::Instant::now();
        let r = client
            .get(format!("{base}/f/{h}/video/seg/1.m4s"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE, "窗烧尽 503");
        let retry_after = r
            .headers()
            .get("Retry-After")
            .map(|v| v.to_str().unwrap().to_string());
        let body = r.text().await.unwrap();
        assert!(body.contains("10"), "超界词面携窗值(10s): {body}");
        assert!(
            body.contains(crate::preview::rendition::SEG_RETRY_WORD),
            "「重试」稳定词(T21 词面消费): {body}"
        );
        assert!(body.contains("稍后重试"), "重试词面: {body}");
        assert_eq!(
            retry_after.as_deref(),
            Some("2"),
            "Retry-After 头(重试节律原料)"
        );
        assert_eq!(
            starts.lock().unwrap().as_slice(),
            [0, 4_000],
            "窗过半恰一次远跳重开(start_at = 目标段×段长)"
        );
        assert!(
            t0.elapsed() >= crate::preview::rendition::RENDITION_SEG_WAIT,
            "整窗烧满(注入钟面;重开不放宽总界): {:?}",
            t0.elapsed()
        );
    }

    /// T15b e2e(远跳→重开→段到达链路,start_paused 注入钟):start_at
    /// 敏感伪转码器——seg 路由远段请求(从头会话恒未就绪)→ 窗过半自动
    /// 重开(start_at = 目标段×段长,工厂 starts 记录面证词)→ 重开会话
    /// SEEKED 字节 200 到手;playlist 按新会话重建(自锚列段,旧段不列);
    /// 重开洞域(锚前段号)= 404 止步;已拉段缓存命中零新会话。
    #[tokio::test(start_paused = true)]
    async fn video_route_far_seek_reopens_with_start_at() {
        let (_gate, srv, reg, state, _home) = video_test_server().await;
        let (factory, made, starts) =
            crate::preview::rendition::test_factory_start_at(FakeTranscoder::gated);
        state.rendition_sessions().install_factory(factory);
        let base = format!("http://127.0.0.1:{}", srv.port());
        let tok = reqwest::header::HeaderValue::from_str(&bearer(&srv)).unwrap();
        let client = reqwest::Client::new();
        let (_dir, _path, _size, h) = video_file_and_handle(&reg);

        // 远跳:直取段 2(缓存空、从头会话恒未就绪)→ 重开链路自动换道
        let r = client
            .get(format!("{base}/f/{h}/video/seg/2.m4s"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "远跳重开后目标段到手");
        assert_eq!(
            r.headers().get("Content-Type").map(|v| v.to_str().unwrap()),
            Some("video/iso.segment"),
            "段 MIME(重开会话透传)"
        );
        assert_eq!(
            r.bytes().await.unwrap(),
            &FakeTranscoder::seeked_bytes(1)[..],
            "SEEKED 字节(重开会话段 1 = 查看端段 2,索引空间沿用)"
        );
        assert_eq!(
            starts.lock().unwrap().as_slice(),
            [0, 8_000],
            "源端收到重开请求且 start_at = 目标段×段长(桥面证词)"
        );
        assert_eq!(made.lock().unwrap()[0].cancels(), 1, "旧会话 cancel 恰一次");

        // playlist 按新会话重建(自重开锚列段,旧段不列)
        let r = client
            .get(format!("{base}/f/{h}/video/index.m3u8"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let pl = r.text().await.unwrap();
        assert!(
            pl.contains(&format!(
                "#EXTINF:4.000,\n{base}/f/{h}/video/seg/2.m4s?t={}",
                srv.token()
            )),
            "playlist 自重开锚列段(绝对+令牌): {pl}"
        );
        assert!(!pl.contains("seg/1"), "旧会话段不列(重建面): {pl}");

        // init 段随重开作废覆写(MAP 面字节 = 新会话 moov,SEEKED 标记)
        let r = client
            .get(format!("{base}/f/{h}/video/seg/0.m4s"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "init 段 200");
        assert_eq!(
            r.bytes().await.unwrap(),
            &FakeTranscoder::seeked_bytes(0)[..],
            "init = 重开会话产出(旧会话 moov 已作废覆写)"
        );

        // 重开洞域(锚前段 1)= 404 止步
        let r = client
            .get(format!("{base}/f/{h}/video/seg/1.m4s"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "洞域止步");
        assert!(r.text().await.unwrap().contains("越界"), "止步词面");

        // 缓存命中:段 2 再取零新会话(字节一致)
        let r = client
            .get(format!("{base}/f/{h}/video/seg/2.m4s"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "已拉段缓存命中");
        assert_eq!(r.bytes().await.unwrap(), &FakeTranscoder::seeked_bytes(1)[..]);
        assert_eq!(starts.lock().unwrap().as_slice(), [0, 8_000], "命中零新会话");
    }

    /// T15b 评审 Important #1 钉面:seg/0(MAP 面)请求**不触发远跳重开**
    /// ——慢产 init(窗内未达)走烧窗 503+seg_retry 诚实面(start_at = 0×
    /// 段长 = 0 的重开即 origin=0,毒化三映射不变量:map_segment 的 0 分
    /// 支遮蔽锚 / advance_pulled 停摆 / playlist MAP+EXTINF 双列 seg/0 →
    /// 永久空播无自愈);内容段映射不受污染(origin 仍 1,seg/1 = 会话段 1)。
    #[tokio::test(start_paused = true)]
    async fn video_route_slow_init_burns_window_without_reopen() {
        let (gate, srv, reg, state, _home) = video_test_server().await;
        let (factory, _made, starts) =
            crate::preview::rendition::test_factory_start_at(|_| FakeTranscoder::init_pending());
        state.rendition_sessions().install_factory(factory);
        let base = format!("http://127.0.0.1:{}", srv.port());
        let tok = reqwest::header::HeaderValue::from_str(&bearer(&srv)).unwrap();
        let client = reqwest::Client::new();
        let (_dir, _path, _size, h) = video_file_and_handle(&reg);

        // seg/0:慢产 init 烧满窗 = 503 seg_retry(零重开)
        let t0 = tokio::time::Instant::now();
        let r = client
            .get(format!("{base}/f/{h}/video/seg/0.m4s"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "慢产 init 烧窗 503(非重开换道)"
        );
        let retry_after = r
            .headers()
            .get("Retry-After")
            .map(|v| v.to_str().unwrap().to_string());
        let body = r.text().await.unwrap();
        assert!(
            body.contains(crate::preview::rendition::SEG_RETRY_WORD),
            "seg_retry 稳定词: {body}"
        );
        assert_eq!(retry_after.as_deref(), Some("2"), "Retry-After 头");
        // 烧满窗下界照钉(注入钟面);**上界不钉**(start_paused 下真实
        // TCP I/O 等待会让 tokio 自动推钟,注册表 sweeper 的 20s interval
        // tick 可把钟面推过 11s 界——双态竞速非行为回归)。回归价值由
        // starts==[0] + origin==1 + seg/1 字节断言承载。
        assert!(
            t0.elapsed() >= crate::preview::rendition::RENDITION_SEG_WAIT,
            "烧满窗(下界;上界不钉见注): {:?}",
            t0.elapsed()
        );
        assert_eq!(
            starts.lock().unwrap().as_slice(),
            [0],
            "seg/0 请求零重开(start_at=0 重开即毒化,缺 index!=0 门即红)"
        );

        // origin 未污染:seg/1 = 会话段 1(origin=0 毒化面即 j=2 → 字节红)
        let r = client
            .get(format!("{base}/f/{h}/video/seg/1.m4s"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "内容段照常");
        assert_eq!(
            r.bytes().await.unwrap(),
            &PseudoTranscoder::segment_bytes(1)[..],
            "origin 仍 1(seg/1 = 会话段 1,毒化面 j=2 即红)"
        );
        // 装配面直读:重开锚仍 1
        let origin = gate
            .video_sessions()
            .lock()
            .unwrap()
            .get(&h)
            .map(|v| v.origin());
        assert_eq!(origin, Some(1), "重开锚未被毒化(origin=1)");
    }

    /// T15-M2 一行修(MIME 毒值不崩):段 Content-Type 装配对非法 header
    /// 字节(远端毒值)fallback fMP4 稳定 MIME——旧形 expect 毒值即
    /// panic(远端可控输入打崩查看端单请求面);合法值原样透传(http
    /// 面合法的 obs-text 多字节 UTF-8 照透,不崩即达标)。
    #[test]
    fn segment_mime_header_falls_back_on_poison() {
        let legit = "video/iso.segment";
        assert_eq!(
            segment_mime_header(legit),
            HeaderValue::from_static(legit),
            "合法透传"
        );
        // http 面合法(obs-text):照透不崩(单请求面诚实降级即可)
        assert_eq!(
            segment_mime_header("视频/毒值"),
            HeaderValue::from_str("视频/毒值").unwrap(),
            "obs-text 合法字节透传(不 panic)"
        );
        // 控制字符毒值:fallback fMP4 稳定 MIME(不 panic)
        for poison in [
            "video/iso.segment\r\nX-Evil: 1", // header 注入(CR/LF)
            "video\x00poison",                // NUL 控制符
            "video\x07bell",                  // 其他控制符
        ] {
            assert_eq!(
                segment_mime_header(poison),
                HeaderValue::from_static("video/iso.segment"),
                "毒值 fallback(不 panic): {poison:?}"
            );
        }
    }

    /// T15 面:源端无转码后端(unsupported)= 404 回退面(客户端回退
    /// 原流+海报;index/seg 两路由同面)。
    #[tokio::test]
    async fn video_route_unsupported_falls_back() {
        let (_gate, srv, reg, _state, _home) = video_test_server().await;
        // 不装工厂 = platform_backend(恒 None)→ open 回 unsupported
        let base = format!("http://127.0.0.1:{}", srv.port());
        let tok = reqwest::header::HeaderValue::from_str(&bearer(&srv)).unwrap();
        let client = reqwest::Client::new();
        let (_dir, _path, _size, h) = video_file_and_handle(&reg);
        for url in [
            format!("{base}/f/{h}/video/index.m3u8"),
            format!("{base}/f/{h}/video/seg/0.m4s"),
        ] {
            let r = client
                .get(&url)
                .header("Authorization", &tok)
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::NOT_FOUND, "{url} 404 回退面");
            let body = r.text().await.unwrap();
            assert!(body.contains("回退"), "词面携回退语义: {body}");
        }
    }

    /// T15 面:「关预览=会话随 handle 释放」(spec §7)——①preview_close
    /// 同语义面(registry.close + gate.detach_video)即回收源端会话;
    /// ②只 close 注册表(漏 detach)时视频路由 410 面照摘 + 后台 detach。
    #[tokio::test]
    async fn video_session_detaches_on_close_and_route_gone() {
        let (gate, srv, reg, state, _home) = video_test_server().await;
        let (factory, _made) =
            crate::preview::rendition::test_factory(FakeTranscoder::standard);
        state.rendition_sessions().install_factory(factory);
        let base = format!("http://127.0.0.1:{}", srv.port());
        let tok = reqwest::header::HeaderValue::from_str(&bearer(&srv)).unwrap();
        let client = reqwest::Client::new();
        let (_dir, _path, _size, h) = video_file_and_handle(&reg);

        // 视频请求即装配会话(源端在册)
        let r = client
            .get(format!("{base}/f/{h}/video/index.m3u8"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            state.rendition_sessions().live_sessions(),
            1,
            "视频请求即装配源端会话"
        );

        // preview_close 同语义面:close + detach_video → 源端会话回收
        assert_eq!(gate.registry().close(&h), CloseOutcome::Removed);
        gate.detach_video(&h);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while state.rendition_sessions().live_sessions() > 0 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "detach 未在窗内回收源端会话"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // 路由 410 面的装配摘除:新句柄 attach 后只 close 注册表,视频
        // 路由 = 410 且后台 detach 照收
        let (_d2, _p2, _s2, h2) = video_file_and_handle(&reg);
        let r = client
            .get(format!("{base}/f/{h2}/video/seg/0.m4s"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "新句柄段面照常");
        assert_eq!(state.rendition_sessions().live_sessions(), 1);
        assert_eq!(gate.registry().close(&h2), CloseOutcome::Removed);
        let r = client
            .get(format!("{base}/f/{h2}/video/seg/0.m4s"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::GONE, "关后视频路由 410");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while state.rendition_sessions().live_sessions() > 0 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "410 面后台 detach 未在窗内回收"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
