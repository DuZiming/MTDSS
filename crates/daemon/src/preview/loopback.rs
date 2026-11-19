//! 预览回环 HTTP 服务(V4-1 T4):仅绑 127.0.0.1 随机口的本地数据面。
//!
//! 路由全景(brief 钉死):`GET /f/{h}/raw`(Range → 206/200/416)、
//! `/f/{h}/thumb?w=`(本任务占位 501,T7 实装)、`/f/{h}/jpeg`(占位 404,
//! T6 实装)、`/f/{h}/video/index.m3u8` 与 `/f/{h}/video/seg/{n}.m4s`
//! (占位 404,T15 实装)。鉴权 `Authorization: Bearer <token>`(token 经
//! preview_open IPC 交付本机 UI,spec §2)缺/错 = 401,**先于**一切路由/
//! handle 判定。handle 未知/过期/已关 = 410(Gone;不区分「从无」与「已
//! 失效」,不探测存在性)。Range 非法矩阵:off ≥ size / len=0(suffix 0)
//! = 416(携 `Content-Range: bytes */size`);畸形 header(非 bytes= 单位/
//! 多段/逆序/非数字)= 400。
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
    cache: Arc<Readahead>,
    /// 转码结果落盘缓存(T5;None = 未接线——缺缓存只损失命中面,转码
    /// 照常。类型平台无关,非 Windows / 未启用 heic 时不消费)。
    #[cfg_attr(
        not(all(windows, feature = "heic")),
        allow(dead_code) // 占位构建:消费面(/jpeg 缓存命中)未编译
    )]
    disk: Option<Arc<super::cache::PreviewDiskCache>>,
}

/// 全 daemon 单例门:惰性起服务(首枚 preview_open 才 bind),重复取用
/// 同一实例。tokio OnceCell 持锁跨 await(仅首启),取用无锁。
pub(crate) struct PreviewGate {
    registry: Arc<HandleRegistry>,
    /// 转码落盘缓存(T5;state.rs 装配注入——缓存构造需要注册表句柄,
    /// 「先门后缓存」的构造序决定走注入面;首枚 preview_open 前必已注入,
    /// 未注入(None)只损失 /jpeg 缓存命中,不损正确性)。
    disk: std::sync::OnceLock<Arc<super::cache::PreviewDiskCache>>,
    server: tokio::sync::OnceCell<Arc<PreviewServer>>,
}

impl PreviewGate {
    pub(crate) fn new() -> Self {
        Self {
            registry: Arc::new(HandleRegistry::new()),
            disk: std::sync::OnceLock::new(),
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

    /// 服务就绪(惰性首启;失败上抛,IPC 面映业务错)。
    pub(crate) async fn server(&self) -> io::Result<Arc<PreviewServer>> {
        let disk = self.disk.get().cloned();
        self.server
            .get_or_try_init(|| PreviewServer::start(Arc::clone(&self.registry), disk))
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
/// 预读缓存都活在路由 state [`SrvCtx`],本结构只是凭据投影);**并持两个
/// 后台任务的句柄**(V4-1 T4 评审 Important #2:serve 与清扫任务不得
/// 脱管——daemon 收口时 abort,不留泄漏任务/残留监听)。
pub(crate) struct PreviewServer {
    port: u16,
    token: String,
    /// axum serve 任务句柄(abort 即断监听;在途连接随 daemon 收口一并断
    /// ——整个 daemon 都在收口,无半程态面)。
    serve: tokio::task::JoinHandle<()>,
    /// 清扫任务句柄(见 [`HandleRegistry::spawn_sweeper`])。
    sweeper: tokio::task::JoinHandle<()>,
}

impl PreviewServer {
    /// 起服务:bind 127.0.0.1:0(OS 随机口),失败换口重试
    /// ([`BIND_ATTEMPTS`] 次);token 每实例一枚(32B 随机 hex);registry
    /// 的常驻清扫任务随启挂起(句柄归本结构,收口 abort)。`disk` = T5
    /// 落盘缓存(/jpeg 转码结果面;None = 未接线,见 [`SrvCtx::disk`])。
    pub(crate) async fn start(
        registry: Arc<HandleRegistry>,
        disk: Option<Arc<super::cache::PreviewDiskCache>>,
    ) -> io::Result<Arc<PreviewServer>> {
        use rand::RngCore as _;
        let mut tok_buf = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut tok_buf);
        let token = hex::encode(tok_buf);
        let cache = Arc::new(Readahead::new());
        let app = router(Arc::new(SrvCtx {
            registry: Arc::clone(&registry),
            token: token.clone(),
            cache: Arc::clone(&cache),
            disk,
        }));
        let mut last_err: Option<io::Error> = None;
        for _ in 0..BIND_ATTEMPTS {
            match TcpListener::bind("127.0.0.1:0").await {
                Ok(listener) => {
                    let port = listener.local_addr()?.port();
                    let sweeper = registry.spawn_sweeper();
                    let serve = tokio::spawn(async move {
                        if let Err(e) = axum::serve(listener, app).await {
                            tracing::warn!(%e, "预览回环服务退出");
                        }
                    });
                    return Ok(Arc::new(PreviewServer {
                        port,
                        token,
                        serve,
                        sweeper,
                    }));
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| io::Error::other("回环口绑定反复失败")))
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    /// 收口:abort serve 与清扫任务(评审 Important #2;调用面 =
    /// [`PreviewGate::shutdown`] ← `Daemon::shutdown`)。abort 是两任务的
    /// 净收口面——serve future 即弃(监听器释放、在途连接断),清扫循环
    /// 无 I/O 半程态;句柄不 await(shutdown 主链路不挂第三方任务收尾,
    /// 取消在下个让渡点即生效)。
    fn abort_tasks(&self) {
        self.serve.abort();
        self.sweeper.abort();
    }

    /// 两后台任务是否都已终局(单测锚:收口断言面)。
    #[cfg(test)]
    fn tasks_finished(&self) -> bool {
        self.serve.is_finished() && self.sweeper.is_finished()
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

/// Bearer 鉴权缝:缺/错 = 401(先于路由与 handle 判定;token 全等比对——
/// 回环面无时序侧信道预算)。
async fn auth(
    State(ctx): State<Arc<SrvCtx>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let ok = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == format!("Bearer {}", ctx.token));
    if !ok {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(req).await
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
                let source = Arc::clone(&self.entry.source);
                match self
                    .cache
                    .block(self.entry.id, block_idx, source, self.entry.size)
                    .await
                {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::warn!(%e, "预览流中块读失败,截断收尾");
                        return None;
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

/// `GET /f/{h}/thumb?w=`:占位 501(T7 实装缩略图后端后透传)。
async fn thumb(
    State(_ctx): State<Arc<SrvCtx>>,
    Path(_h): Path<String>,
    Query(_q): Query<HashMap<String, String>>,
) -> Response {
    (StatusCode::NOT_IMPLEMENTED, "缩略图后端未实装(T7)").into_response()
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

/// `GET /f/{h}/video/index.m3u8`:占位 404(T15 实装 HLS 装配)。
async fn hls_index(State(_ctx): State<Arc<SrvCtx>>, Path(_h): Path<String>) -> Response {
    (StatusCode::NOT_FOUND, "视频面未实装(T15)").into_response()
}

/// `GET /f/{h}/video/seg/{seg}`(seg 含 `.m4s` 后缀):占位 404(T15 实装)。
async fn hls_seg(
    State(_ctx): State<Arc<SrvCtx>>,
    Path((_h, _seg)): Path<(String, String)>,
) -> Response {
    (StatusCode::NOT_FOUND, "视频面未实装(T15)").into_response()
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

    /// 测试服务(独立 registry + 127.0.0.1 随机口;无落盘缓存接线)。
    async fn test_server() -> (Arc<PreviewServer>, Arc<HandleRegistry>) {
        let reg = Arc::new(HandleRegistry::new());
        let srv = PreviewServer::start(reg.clone(), None)
            .await
            .expect("起服务");
        (srv, reg)
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
        let srv = PreviewServer::start(reg.clone(), Some(Arc::clone(&disk)))
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

        // 占位路由:thumb 501 / jpeg 404 / video 404(鉴权后)
        let r = client
            .get(format!("{base}/f/{h}/thumb?w=256"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::NOT_IMPLEMENTED,
            "thumb 占位 501(T7)"
        );
        let r = client
            .get(format!("{base}/f/{h}/jpeg"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "jpeg 占位 404(T6)");
        let r = client
            .get(format!("{base}/f/{h}/video/index.m3u8"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::NOT_FOUND,
            "video playlist 占位 404(T15)"
        );
        let r = client
            .get(format!("{base}/f/{h}/video/seg/3.m4s"))
            .header("Authorization", &tok)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "video seg 占位 404(T15)");

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
        let srv = PreviewServer::start(reg.clone(), None)
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
}
