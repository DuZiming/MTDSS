//! V4-1 预览波:引擎侧预览数据面(T3)。
//!
//! `file_read_range` 的 CtrlReply 级处理器([`read_range_reply`]):响应方
//! 解析请求(设备自校验 → 服务桥 vpath seek 源桥先行/导出回退 · 照片
//! vpath 同面(T8 手机 seek fd 桥)/本机 open_source → 声明大小
//! 核对;授权根守卫在分发缝 [`handle_ctrl_request_reply`] 已过,serve_path
//! vpath 直通臂对齐 List/Stat 口径)→ 独立限流
//! ([`limiter::PreviewLimiter`],浏览>预览>批量传输)→ 有界通道生产任务
//! 逐段读 [`source::FileSource`] → core 流式泵(DataBegin/DataChunk*/
//! DataEnd,T2 基座)。
//!
//! 错误面统一 `source_offline` 稳定码(源打开失败/桥导出失败/导出件越界/
//! 流中读败——发起端据此走下载回退);NOT_FOUND 仅用于「文件已变更(size
//! 不符)」与「设备串台」两个请求方自愈面;帽满拒绝帧沿用 T2 的 INTERNAL
//! (发起端不做依赖码区分的重试)。
//!
//! 纪律(T2 评审转达,逐条落点):①生产端通道有界([`STREAM_WINDOW`] ×
//! [`READ_CHUNK`],背压沿 T2 泵链逐级回压);②Sender 不出生产任务
//! (DataEnd = 通道关 = 泵收尾;泵亡即 send 败自收);③空段零帧(len=0/
//! EOF 即 begin+end,段界不以帧计数承载);④流中失败 CtrlError 帧收尾,
//! 不再有 DataEnd。

/// 预览落盘缓存(V4-1 T5):`{home}/preview-tmp` + 全局 4 GiB 磁盘 LRU
/// (office 整取等全文件预览面;语义见 [`cache`])。
pub(crate) mod cache;
#[cfg(all(windows, feature = "heic"))]
pub(crate) mod heic;
pub(crate) mod handles;
pub(crate) mod limiter;
pub(crate) mod loopback;
/// 转码会话引擎面(V4-1 T12):源端会话管理(空闲 60s 回收)+ Transcoder
/// 后端 trait(v1 桌面缺席 → open 回 unsupported)+ 查看端装配骨架
/// (拉段入 T5 缓存 + HLS playlist 合成);rendition 三 op 的 ctrl 应答臂
/// 在分发缝置换 T1 桩。语义见 [`rendition`]。
pub(crate) mod rendition;
pub(crate) mod source;
/// 源端缩略图引擎面+缓存(V4-1 T7):`thumb(path,mtime,size,w)->Vec<u8>`
/// (JPEG;失败/无后端=空字节)+ `{home}/thumb-tmp` 512 MiB LRU + ctrl
/// Thumb 应答([`thumb::thumb_reply`]);语义见 [`thumb`]。
pub(crate) mod thumb;
/// Windows 缩略图后端(V4-1 T7):Shell IShellItemImageFactory(COM)。
#[cfg(windows)]
pub(crate) mod thumb_win;

use mtdss_core::transport::ctrl::{CtrlChunk, CtrlReply, CtrlStreamError, CtrlStreamOut};
use mtdss_proto::ctrl::{CtrlRequest, CtrlResponse, FileReadRangeRequest};
use std::sync::Arc;

use crate::ctrl::err_response;
use crate::state::DaemonState;
use limiter::PreviewClass;
use source::FileSource;

/// 单次 read_at 的读段帽(生产端读缓冲粒度;与线上
/// `STREAM_CHUNK_CAP` 同值不同责——本帽管读粒度,线上帽管帧界)。
/// pub(crate):rendition 段泵同值复用(段读粒度)。
pub(crate) const READ_CHUNK: usize = 256 * 1024;

/// 生产端通道窗口(块数;应用级窗口 = 窗口×READ_CHUNK 字节。消费端不读
/// 即在此背压到 spawn_blocking 读源,沿 T2「通道满 → 泵 write_all 阻塞 →
/// QUIC 流控」反向逐级回压)。pub(crate):rendition 段泵同值复用。
pub(crate) const STREAM_WINDOW: usize = 4;

/// ctrl 请求的流式应答分发缝(serve 面两臂——裸 serve_pooled 与 mux
/// TAG_CTRL——统一消费):版本吻合的 `file_read_range` 走流式处理器
/// ([`read_range_reply`])、`thumb` 走缩略图应答([`thumb::thumb_reply`],
/// 恒单帧——失败=空字节,不回错误帧)、`rendition_open/segment/cancel`
/// 走转码会话三臂([`rendition`] T12:open 单帧/段流式/cancel 空帧;
/// 守卫在此缝过 guard_local_paths——open 路径臂对齐 FileReadRange/Thumb
/// 口径,segment/cancel 无本机路径面直通);其余(含版本不符/其余变体)
/// 沿既有单帧分发,线上字节逐字节不变。
pub(crate) async fn handle_ctrl_request_reply(
    state: Arc<DaemonState>,
    req: CtrlRequest,
) -> CtrlReply {
    use mtdss_proto::ctrl::ctrl_request::Body as Req;
    if req.version == mtdss_proto::CTRL_VERSION
        && matches!(
            req.body.as_ref(),
            Some(Req::FileReadRange(_))
                | Some(Req::Thumb(_))
                | Some(Req::RenditionOpen(_))
                | Some(Req::RenditionSegment(_))
                | Some(Req::RenditionCancel(_))
        )
    {
        if let Err(msg) = crate::ctrl::guard_local_paths(&state, req.body.as_ref()) {
            return CtrlReply::Frame(err_response("INVALID_PATH", msg));
        }
        return match req.body.as_ref() {
            Some(Req::FileReadRange(r)) => read_range_reply(state, r.clone()).await,
            Some(Req::Thumb(t)) => thumb::thumb_reply(state, t.clone()).await,
            // RenditionSegment/RenditionCancel 为全标量 Copy 形(prost 派生
            // 与 FileReadRange/Thumb 的 Clone 面同因,此处按值即可)
            Some(Req::RenditionOpen(r)) => rendition::rendition_open_reply(state, r.clone()).await,
            Some(Req::RenditionSegment(r)) => {
                rendition::rendition_segment_reply(state, *r).await
            }
            Some(Req::RenditionCancel(c)) => rendition::rendition_cancel_reply(state, *c).await,
            _ => unreachable!("matches! 分支已证预览五变体"),
        };
    }
    CtrlReply::Frame(crate::ctrl::handle_ctrl_request(state, req).await)
}

/// `file_read_range` 的流式应答(守卫已过;错误以单帧 CtrlError 回执,
/// 数据以 Stream 臂回——T2 泵 begin/chunk*/end)。
pub(crate) async fn read_range_reply(
    state: Arc<DaemonState>,
    r: FileReadRangeRequest,
) -> CtrlReply {
    // 设备自校验(防串台,proto 字段注):请求所标源设备非本机 = 路由
    // 陈旧/错投,NOT_FOUND 诚实拒(发起端按设备目录重投)。
    if Some(r.device_id.as_str()) != state.self_device_id().as_deref() {
        return CtrlReply::Frame(err_response(
            "NOT_FOUND",
            format!("目标设备不符(防串台): {}", r.device_id),
        ));
    }
    // 路由:服务桥 vpath → seek 源桥先行(T8 fd 直读)→ 既有导出件回退
    // (serve.export 导轨复用,ExportFileSource 挂清理生命周期);照片
    // vpath → 照片臂(T8:seek 源桥先行 → photo.export 导出件回退);其余
    // = 本机路径 std::fs 主路径(open_source)。单快照路由(守卫同款纪律
    // ——分发缝 guard 与此处 route 各取一次,roots 热换竞窗下双读可换
    // 集,两读不一致时以分发缝守卫为准:守卫放行集内/桥内路径,此处
    // route 未命中即落 fs 面,由 fs 的 NOT_FOUND 诚实报错,不挂死)。
    let opened = match state.guard().bridge_route(&r.path) {
        Some(hit) => route_bridge(&state, &r.path, &hit).await,
        None if crate::photo::photo_vpath(&r.path).is_some() => route_photo(&state, &r.path).await,
        None => match source::open_source(&r.device_id, &r.path, r.mtime, r.size as u64) {
            Ok(src) => Ok(src),
            Err(e) => Err(err_response("source_offline", format!("源打开失败: {e}"))),
        },
    };
    let source = match opened {
        Ok(s) => s,
        Err(frame) => return CtrlReply::Frame(frame),
    };
    // 声明大小核对(发起端以 (path,size) 作预览缓存键,proto 注):声明
    // 在场且与源不符 = 文件已变更,NOT_FOUND(请求方重新浏览取新键;照读
    // 会污染旧键缓存)。
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
    // 独立限流:预览档令牌(浏览>预览>批量传输;低档等待老化晋升防饿死)
    // ——令牌随生产任务终了归还(读面并发,非流生命周期)。
    let permit = state.preview_limiter().acquire(PreviewClass::Preview).await;
    // 有界通道 + 生产任务(Sender 不出任务——DataEnd=通道关;泵亡即 send
    // 败自收;len=0/EOF = 零数据帧)
    let (tx, rx) = tokio::sync::mpsc::channel::<CtrlChunk>(STREAM_WINDOW);
    tokio::spawn(produce_chunks(source, permit, tx, r.offset, r.len as u64));
    CtrlReply::Stream(CtrlStreamOut { chunks: rx })
}

/// 服务桥 vpath 路由(T8 手机 seek 源桥):**seek fd 直读先行**——
/// `fs.request_seek` → App 交付可 seek fd(`FsFileSource::from_seek_fd`
/// 收编,视频随机读面免整件导出);桥不可用/超时/拒开/不可 seek 一律落
/// **既有导出件回退**(serve.export 导轨复用,轨 T3 已备,勿重写——桌面
/// daemon 无 fd 通道占位恒 Err,行为与泛化前逐字节一致)。pub(crate):
/// rendition open 应答臂(T12)同轨复用(单快照路由同缝)。
pub(crate) async fn route_bridge(
    state: &DaemonState,
    path: &str,
    hit: &crate::roots::BridgeHit,
) -> Result<Arc<dyn FileSource>, CtrlResponse> {
    if let Ok(src) = seek_source(state, path).await {
        return Ok(src);
    }
    tracing::debug!(path, "seek 源桥不可用,落导出件回退");
    let (_temp, export) = route_bridge_export(state, &hit.root, &hit.sub).await?;
    Ok(Arc::new(export))
}

/// seek 源桥读面(T8):`fs.request_seek` → App 交付 fd → 可 seek 源。
/// Err = 桥不可用(无订阅/无 fd 通道)/超时/拒开/不可 seek(词面仅日志
/// 消费,回退不依赖文案)。
async fn seek_source(state: &DaemonState, path: &str) -> Result<Arc<dyn FileSource>, String> {
    let file = state
        .jobs()
        .seek_bridge()
        .open(path)
        .await
        .map_err(|e| format!("seek 源桥不可用: {e}"))?;
    let src = source::FsFileSource::from_seek_fd(file)
        .map_err(|e| format!("seek 源不可用: {e}"))?;
    Ok(Arc::new(src))
}

/// 照片 vpath 路由(T8,SAF/MediaStore 两源的手机面):seek fd 直读先行
/// (App 按 `/照片图库` 前缀分流 MediaStore openFileDescriptor)→ 既有
/// 照片导出回退——photo.stat_with_id 单查(photo_id 导出凭据)→
/// photo.export 导出件(`{home}/photo-serve/`)→ ExportFileSource(桥
/// vpath 导出回退同形;回传路径越界 fail-closed)。photo 桥缺席/单查失
/// 败/导出失败 = source_offline(对齐服务桥回退词面口径)。pub(crate):
/// rendition open 应答臂(T12)同轨复用(单快照路由同缝)。
pub(crate) async fn route_photo(state: &DaemonState, path: &str) -> Result<Arc<dyn FileSource>, CtrlResponse> {
    if let Ok(src) = seek_source(state, path).await {
        return Ok(src);
    }
    tracing::debug!(path, "照片 seek 源桥不可用,落导出件回退");
    let Some(photo) = state.photo() else {
        return Err(err_response(
            "source_offline",
            "照片桥未启用(photo_serve 关)".into(),
        ));
    };
    // 单查 + 导出凭据(目录/根 photo_id = None:按需读只对文件成立)
    let (_data, photo_id) = photo
        .stat_with_id(path)
        .await
        .map_err(|e| err_response("source_offline", format!("照片源不可用: {e}")))?;
    let Some(photo_id) = photo_id else {
        return Err(err_response(
            "source_offline",
            format!("目录不可按需读(仅文件): {path}"),
        ));
    };
    let temp = photo
        .export(&photo_id)
        .await
        .map_err(|e| err_response("source_offline", format!("照片导出失败: {e}")))?;
    if crate::photo::path_under_serve_dir(&state.home(), &temp) {
        let prefix = crate::photo::serve_dir(&state.home());
        source::ExportFileSource::open(temp, Some(prefix))
            .map(|s| Arc::new(s) as Arc<dyn FileSource>)
            .map_err(|e| err_response("source_offline", format!("导出件打开失败: {e}")))
    } else {
        Err(err_response(
            "source_offline",
            format!(
                "App 照片导出失败: 回传路径越出照片临时目录({})",
                temp.display()
            ),
        ))
    }
}

/// 服务桥导出的「临时件路径 + 可 seek 源」双面(T3 读面与 T7 缩略图面
/// 共用):读面只要源;缩略图面(Shell 后端按**路径**开文件)两样都要
/// ——临时件路径喂后端,源手柄活到生成终了(Drop 清理临时件)。
async fn route_bridge_export(
    state: &DaemonState,
    root: &str,
    sub: &str,
) -> Result<(std::path::PathBuf, source::ExportFileSource), CtrlResponse> {
    let Some(serve) = state.serve() else {
        return Err(err_response(
            "INVALID_PATH",
            "服务桥未启用(无 bridge 根注册)".into(),
        ));
    };
    match serve.export(root, sub).await {
        Ok(temp) if crate::serve::path_under_serve_dir(&state.home(), &temp) => {
            let prefix = crate::serve::serve_dir(&state.home());
            source::ExportFileSource::open(temp.clone(), Some(prefix))
                .map(|s| (temp, s))
                .map_err(|e| err_response("source_offline", format!("导出件打开失败: {e}")))
        }
        Ok(temp) => Err(err_response(
            "source_offline",
            format!(
                "App 服务导出失败: 回传路径越出服务临时目录({})",
                temp.display()
            ),
        )),
        Err(e) => Err(err_response(
            "source_offline",
            format!("服务目录不可用: {e}"),
        )),
    }
}

/// 生产任务:逐段 read_at(spawn_blocking 落阻塞池)→ 有界通道;EOF/读尽
/// 即通道关(= DataEnd);读败 → `source_offline` 错误块收尾(泵改写
/// CtrlError 帧,不再有 DataEnd)。permit 持至任务终(读面令牌);泵亡
/// (取消/写败收流)即 send 败自收,令牌随 permit 落放——Sender 恒不出
/// 本任务,无克隆泄漏面。
async fn produce_chunks(
    source: Arc<dyn FileSource>,
    _permit: limiter::PreviewPermit,
    tx: tokio::sync::mpsc::Sender<CtrlChunk>,
    mut offset: u64,
    mut remaining: u64,
) {
    while remaining > 0 {
        let want = remaining.min(READ_CHUNK as u64) as usize;
        let src = Arc::clone(&source);
        // 读段(spawn_blocking:std-fs seek+read 是阻塞调用,直跑占死
        // 异步 worker——blocking_fs 同款纪律)
        let read = tokio::task::spawn_blocking(move || {
            let mut buf = vec![0u8; want];
            let n = src.read_at(offset, &mut buf)?;
            buf.truncate(n);
            Ok::<_, std::io::Error>(buf)
        })
        .await;
        match read {
            Ok(Ok(buf)) if buf.is_empty() => break, // EOF:剩余不再读,空段零帧
            Ok(Ok(buf)) => {
                let n = buf.len() as u64;
                if tx.send(Ok(buf)).await.is_err() {
                    return; // 泵亡:流已收,令牌随 permit 落放
                }
                offset += n;
                remaining -= n;
            }
            Ok(Err(e)) => {
                let _ = tx
                    .send(Err(CtrlStreamError {
                        code: "source_offline".into(),
                        message: format!("源读取失败: {e}"),
                    }))
                    .await;
                return;
            }
            Err(join) => {
                let _ = tx
                    .send(Err(CtrlStreamError {
                        code: "INTERNAL".into(),
                        message: format!("读源任务收尾失败: {join}"),
                    }))
                    .await;
                return;
            }
        }
    }
}

/// 请求构造小助(单测面):self 语义的 device_id 由调用方填(源端自校验
/// 以真实 device_id 认)。
#[cfg(test)]
pub(crate) fn read_range_request(
    device_id: &str,
    path: &str,
    mtime: i64,
    size: u64,
    offset: u64,
    len: u32,
) -> CtrlRequest {
    CtrlRequest {
        version: mtdss_proto::CTRL_VERSION,
        body: Some(mtdss_proto::ctrl::ctrl_request::Body::FileReadRange(
            FileReadRangeRequest {
                device_id: device_id.to_string(),
                path: path.to_string(),
                mtime,
                size: size as i64,
                offset,
                len,
            },
        )),
    }
}

/// 缩略图请求构造小助(单测面;T7 分发缝用例)。
#[cfg(test)]
fn thumb_request(path: &str, mtime: i64, size: u64, width: u32) -> CtrlRequest {
    CtrlRequest {
        version: mtdss_proto::CTRL_VERSION,
        body: Some(mtdss_proto::ctrl::ctrl_request::Body::Thumb(
            mtdss_proto::ctrl::ThumbRequest {
                path: path.to_string(),
                mtime,
                size: size as i64,
                width,
            },
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbox_dest;
    use crate::roots::{RootSource, RootSpec};
    use crate::state::DaemonState;
    use mtdss_proto::ctrl::ctrl_response;

    /// 预置会话(测试态单点):预览读的设备自校验需要 self_device_id,
    /// 全新 home 无 session = None——先落 session.json 再装配 state。
    fn seed_session(home: &std::path::Path) -> String {
        let session = crate::session::Session {
            server: "http://127.0.0.1:9".into(),
            jwt: "test".into(),
            refresh: "test".into(),
            device_id: "selfdev".into(),
            ca: None,
        };
        session.save(home).unwrap();
        session.device_id
    }

    /// 测试态(独立临时 home;ctrl.rs/mux.rs tests 同款装配面单点;预置
    /// 会话供设备自校验)。
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

    /// 图样(写面与断言面同源)。
    fn pattern(i: u64) -> u8 {
        (i.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as u8
    }

    /// 直驱消费:从 CtrlStreamOut 的通道收齐全量(单测面无泵——生产块即
    /// recv 项)。
    async fn drain(mut rx: tokio::sync::mpsc::Receiver<CtrlChunk>) -> (Vec<u8>, usize) {
        let mut bytes = Vec::new();
        let mut n = 0usize;
        while let Some(item) = rx.recv().await {
            match item {
                Ok(c) => {
                    bytes.extend_from_slice(&c);
                    n += 1;
                }
                Err(e) => panic!("流中失败(用例未预期): {e:?}"),
            }
        }
        (bytes, n)
    }

    /// 单帧应答的 (ok, code) 投影(Stream 臂 / 非 Error 体 = None;断言
    /// 面只钉稳定码,不钉文案)。
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

    /// 全量读:字节一致 + 读面块数(300KiB = 262_144+37_856 两块;线上
    /// 三帧拆分归泵,由 ctrl_e2e 行为面钉)。
    #[tokio::test]
    async fn read_range_streams_full_bytes_in_two_producer_chunks() {
        let state = test_state();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.bin");
        let len: u64 = 300_000;
        let data: Vec<u8> = (0..len).map(pattern).collect();
        std::fs::write(&path, &data).unwrap();
        let self_id = state.self_device_id().unwrap_or_default();

        let reply = read_range_reply(
            state.clone(),
            FileReadRangeRequest {
                device_id: self_id,
                path: path.to_string_lossy().into_owned(),
                mtime: 0,
                size: len as i64,
                offset: 0,
                len: len as u32,
            },
        )
        .await;
        let CtrlReply::Stream(out) = reply else {
            panic!("全量读必须流式");
        };
        let (bytes, chunks) = drain(out.chunks).await;
        assert_eq!(bytes, data, "全量字节一致");
        assert_eq!(chunks, 2, "读面块 = 262_144+37_856 恰两块(READ_CHUNK)");
    }

    /// 子段读:非对齐中段字节一致 + 单生产块。
    #[tokio::test]
    async fn read_range_subsegment_bytes() {
        let state = test_state();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.bin");
        let len: u64 = 100_000;
        let data: Vec<u8> = (0..len).map(pattern).collect();
        std::fs::write(&path, &data).unwrap();
        let self_id = state.self_device_id().unwrap_or_default();

        let reply = read_range_reply(
            state,
            FileReadRangeRequest {
                device_id: self_id,
                path: path.to_string_lossy().into_owned(),
                mtime: 0,
                size: len as i64,
                offset: 73_457,
                len: 10_000,
            },
        )
        .await;
        let CtrlReply::Stream(out) = reply else {
            panic!("子段读必须流式");
        };
        let (bytes, chunks) = drain(out.chunks).await;
        assert_eq!(bytes, data[73_457..83_457], "非对齐中段字节一致");
        assert_eq!(chunks, 1);
    }

    /// 空段零帧纪律(len=0 与 offset==size 都是 begin+end 零数据帧)。
    #[tokio::test]
    async fn read_range_empty_reads_produce_zero_chunks() {
        let state = test_state();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.bin");
        std::fs::write(&path, b"12345").unwrap();
        let self_id = state.self_device_id().unwrap_or_default();
        let path_s = path.to_string_lossy().into_owned();

        // len = 0
        let reply = read_range_reply(
            state.clone(),
            FileReadRangeRequest {
                device_id: self_id.clone(),
                path: path_s.clone(),
                mtime: 0,
                size: 5,
                offset: 0,
                len: 0,
            },
        )
        .await;
        let CtrlReply::Stream(out) = reply else {
            panic!("len=0 必须流式(空数据帧)");
        };
        let (bytes, chunks) = drain(out.chunks).await;
        assert!(bytes.is_empty() && chunks == 0, "len=0 = 零数据帧");

        // offset == size(EOF 探测)
        let reply = read_range_reply(
            state,
            FileReadRangeRequest {
                device_id: self_id,
                path: path_s,
                mtime: 0,
                size: 5,
                offset: 5,
                len: 100,
            },
        )
        .await;
        let CtrlReply::Stream(out) = reply else {
            panic!("EOF 探测必须流式(空数据帧)");
        };
        let (bytes, chunks) = drain(out.chunks).await;
        assert!(bytes.is_empty() && chunks == 0, "EOF 探测 = 零数据帧");
    }

    /// 设备串台拒(NOT_FOUND 单帧):请求所标源设备非本机。
    #[tokio::test]
    async fn read_range_rejects_device_mismatch() {
        let state = test_state();
        let reply = read_range_reply(
            state,
            FileReadRangeRequest {
                device_id: "别家的设备".into(),
                path: "C:\\x.bin".into(),
                mtime: 0,
                size: 0,
                offset: 0,
                len: 1,
            },
        )
        .await;
        let got = frame_code(&reply);
        assert_eq!(
            got,
            Some((false, "NOT_FOUND".into())),
            "串台请求 NOT_FOUND 单帧拒: {got:?}"
        );
    }

    /// 缺失文件 → source_offline 单帧(发起端据此走下载回退)。
    #[tokio::test]
    async fn read_range_missing_file_is_source_offline() {
        let state = test_state();
        let dir = tempfile::tempdir().unwrap();
        let self_id = state.self_device_id().unwrap_or_default();
        let reply = read_range_reply(
            state,
            FileReadRangeRequest {
                device_id: self_id,
                path: dir.path().join("缺.bin").to_string_lossy().into_owned(),
                mtime: 0,
                size: 0,
                offset: 0,
                len: 10,
            },
        )
        .await;
        let got = frame_code(&reply);
        assert_eq!(
            got,
            Some((false, "source_offline".into())),
            "源缺失 source_offline: {got:?}"
        );
    }

    /// 声明大小不符(文件已变更)→ NOT_FOUND 单帧,请求方重新浏览。
    #[tokio::test]
    async fn read_range_size_mismatch_rejected() {
        let state = test_state();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.bin");
        std::fs::write(&path, vec![0u8; 100]).unwrap();
        let self_id = state.self_device_id().unwrap_or_default();
        let reply = read_range_reply(
            state,
            FileReadRangeRequest {
                device_id: self_id,
                path: path.to_string_lossy().into_owned(),
                mtime: 0,
                size: 999, // 陈旧声明
                offset: 0,
                len: 10,
            },
        )
        .await;
        let got = frame_code(&reply);
        assert_eq!(
            got,
            Some((false, "NOT_FOUND".into())),
            "size 不符 NOT_FOUND: {got:?}"
        );
    }

    /// 服务桥 vpath 路由:bridge 根注册(App 缺席 = 零订阅快失败)→
    /// source_offline(导出不可达),而非 fs 面错误——vpath 臂真实路由到
    /// 导出导轨;真桥应答面归 serve_e2e(unix fdpass)。
    #[tokio::test]
    async fn read_range_bridge_vpath_routes_to_export_rail() {
        let (state, _home) = test_state_with_home();
        let root = _home.path().join("桥根");
        std::fs::create_dir_all(&root).unwrap();
        state.set_roots(vec![RootSpec {
            name: "桥".into(),
            path: root.to_string_lossy().into_owned(),
            source: RootSource::Bridge,
            visible: true,
        }]);
        let self_id = state.self_device_id().unwrap_or_default();
        let vpath = format!("{}{}x.bin", root.display(), std::path::MAIN_SEPARATOR);
        let reply = read_range_reply(
            state,
            FileReadRangeRequest {
                device_id: self_id,
                path: vpath,
                mtime: 0,
                size: 0,
                offset: 0,
                len: 10,
            },
        )
        .await;
        let got = frame_code(&reply);
        assert_eq!(
            got,
            Some((false, "source_offline".into())),
            "桥 vpath 走导出轨,App 缺席 = source_offline: {got:?}"
        );
    }

    /// 单帧应答的 (ok, code, message) 投影(路由词面断言面:照片 vpath 的
    /// source_offline 文案携照片桥词,与 fs 臂「源打开失败」可区分)。
    fn frame_msg(reply: &CtrlReply) -> Option<(bool, String, String)> {
        let CtrlReply::Frame(resp) = reply else {
            return None;
        };
        resp.body
            .as_ref()
            .and_then(|b| match b {
                ctrl_response::Body::Error(e) => {
                    Some((resp.ok, e.code.clone(), e.message.clone()))
                }
                _ => None,
            })
    }

    /// 照片 vpath 路由(T8 手机 seek 源桥):照片 vpath 不再落 fs 读臂
    /// (泛化前 = open_source NotFound,词面「源打开失败」),改照片臂
    /// ——seek 源桥先行(无订阅快败)→ 照片导出回退;photo 桥缺席 =
    /// source_offline 携「照片桥未启用」词面。真桥应答面归 seek e2e
    /// (unix fdpass)。(unix 门:photo_vpath 的词法规范化按平台绝对
    /// 形态收口——生产面只在 iOS/Android 引擎,photo_vpath_matrix 同门。)
    #[cfg(unix)]
    #[tokio::test]
    async fn read_range_photo_vpath_routes_photo_rail() {
        let state = test_state();
        let self_id = state.self_device_id().unwrap_or_default();
        let reply = read_range_reply(
            state,
            FileReadRangeRequest {
                device_id: self_id,
                path: "/照片图库/相册/x.jpg".into(),
                mtime: 0,
                size: 0,
                offset: 0,
                len: 10,
            },
        )
        .await;
        let (ok, code, msg) = frame_msg(&reply).expect("照片 vpath 落照片臂 = 单帧错");
        assert_eq!((ok, code.as_str()), (false, "source_offline"));
        assert!(
            msg.contains("照片桥未启用"),
            "词面钉面(照片臂,非 fs 臂): {msg}"
        );
    }

    /// 照片 vpath 路由(photo 桥在场、App 缺席):seek 快败 → 照片导出
    /// 回退 → 单查快败(photo.list 零订阅)→ source_offline 搁照片词。
    /// (unix 门:photo_vpath 平台绝对形态收口,同上。)
    #[cfg(unix)]
    #[tokio::test]
    async fn read_range_photo_vpath_bridge_present_app_absent_is_source_offline() {
        let (events, _) = tokio::sync::broadcast::channel(16);
        let _home = tempfile::tempdir().unwrap();
        let state = {
            // photo_serve 开的测试态(照片桥随 state 常驻)
            let session = crate::session::Session {
                server: "http://127.0.0.1:9".into(),
                jwt: "test".into(),
                refresh: "test".into(),
                device_id: "selfdev".into(),
                ca: None,
            };
            session.save(_home.path()).unwrap();
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
                    _home.path(),
                    _home.path().join("inbox"),
                    None,
                    inbox_dest::staging_dir(_home.path()),
                    false,
                    true, // photo_serve 开
                    Default::default(),
                    Default::default(),
                    crate::ctrl::ctrl_default_tuning(),
                    events,
                    update,
                )
                .unwrap(),
            )
        };
        let self_id = state.self_device_id().unwrap_or_default();
        let reply = read_range_reply(
            state,
            FileReadRangeRequest {
                device_id: self_id,
                path: "/照片图库/相册/x.jpg".into(),
                mtime: 0,
                size: 0,
                offset: 0,
                len: 10,
            },
        )
        .await;
        let (ok, code, msg) = frame_msg(&reply).expect("照片臂单帧错");
        assert_eq!((ok, code.as_str()), (false, "source_offline"));
        assert!(msg.contains("照片"), "照片桥在场/App 缺席 = 照片词: {msg}");
    }

    /// 授权根守卫(分发缝):FileReadRange 集外路径 INVALID_PATH 单帧拒
    /// (vpath 直通臂不越过授权面——bridge vpath 才直通)。
    #[tokio::test]
    async fn read_range_outside_authorized_roots_rejected() {
        let (state, _home) = test_state_with_home();
        std::fs::create_dir_all(_home.path().join("docs")).unwrap();
        std::fs::create_dir_all(_home.path().join("secret")).unwrap();
        std::fs::write(_home.path().join("secret").join("s.bin"), b"s").unwrap();
        state.set_roots(vec![RootSpec {
            name: "docs".into(),
            path: _home.path().join("docs").to_string_lossy().into_owned(),
            source: RootSource::Fs,
            visible: true,
        }]);
        // 集外路径:经 handle_ctrl_request_reply(分发缝)INVALID_PATH
        let req = read_range_request(
            state.self_device_id().unwrap_or_default().as_str(),
            _home
                .path()
                .join("secret")
                .join("s.bin")
                .to_string_lossy()
                .as_ref(),
            0,
            1,
            0,
            1,
        );
        let reply = handle_ctrl_request_reply(state, req).await;
        let got = frame_code(&reply);
        assert_eq!(
            got,
            Some((false, "INVALID_PATH".into())),
            "集外 FileReadRange INVALID_PATH: {got:?}"
        );
    }

    /// 限流在环:令牌耗尽时新读源排队(2 令牌,第 3 路 await 至有票才进
    /// 数据面);permit 随生产任务终了归还。直驱三路并发,第 3 路首块晚到
    /// 但终局一致。
    #[tokio::test]
    async fn read_range_limits_concurrent_sources() {
        let state = test_state();
        let dir = tempfile::tempdir().unwrap();
        let self_id = state.self_device_id().unwrap_or_default();
        // 建三份小件,先手动占满两枚令牌
        let hold = state.preview_limiter();
        let p1 = hold.try_acquire(PreviewClass::Preview).expect("票一");
        let p2 = hold.try_acquire(PreviewClass::Preview).expect("票二");
        let mut paths = Vec::new();
        for i in 0..3 {
            let p = dir.path().join(format!("f{i}.bin"));
            std::fs::write(&p, vec![i as u8; 10]).unwrap();
            paths.push(p);
        }
        let (tx3, mut rx3) = tokio::sync::mpsc::channel::<CtrlChunk>(2);
        // 第三路读源:acquire 必排队(try_acquire None 已由上两票证明面)
        assert!(
            hold.try_acquire(PreviewClass::Preview).is_none(),
            "两票在手,第三路必排队"
        );
        let state3 = state.clone();
        let dev3 = self_id.clone();
        let path3 = paths[2].to_string_lossy().into_owned();
        tokio::spawn(async move {
            let req = FileReadRangeRequest {
                device_id: dev3,
                path: path3,
                mtime: 0,
                size: 10,
                offset: 0,
                len: 10,
            };
            let CtrlReply::Stream(out) = read_range_reply(state3, req).await else {
                panic!("第三路必须流式");
            };
            let (bytes, _) = drain(out.chunks).await;
            let _ = tx3.send(Ok(bytes)).await;
        });
        // 放一票:第三路获得令牌,进数据面
        drop(p1);
        let got = tokio::time::timeout(std::time::Duration::from_secs(5), rx3.recv())
            .await
            .expect("第三路放票后必须进数据面")
            .expect("通道不早关")
            .expect("无中途错");
        assert_eq!(got, vec![2u8; 10], "第三路字节一致");
        drop(p2);
    }

    /// Thumb 应答投影(单帧面):(ok, jpeg bytes)。
    fn thumb_jpeg(reply: &CtrlReply) -> Option<(bool, Vec<u8>)> {
        let CtrlReply::Frame(resp) = reply else {
            return None;
        };
        resp.body
            .as_ref()
            .and_then(|b| match b {
                ctrl_response::Body::Thumb(t) => Some((resp.ok, t.jpeg.clone())),
                _ => None,
            })
    }

    /// T7 分发缝(本机图片,Windows 真后端):ThumbRequest → 恒单帧
    /// ok=true + 非空 jpeg(image crate 可解);支持类型走引擎面。
    #[cfg(windows)]
    #[tokio::test]
    async fn thumb_reply_local_image_is_ok_jpeg_frame() {
        let _serial = thumb::TEST_SERIAL.lock().await;
        let state = test_state();
        let dir = tempfile::tempdir().unwrap();
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(80, 60, |x, y| {
            image::Rgb([(x * 2 % 256) as u8, (y * 3 % 256) as u8, 60])
        }));
        let path = dir.path().join("pic.jpg");
        image::codecs::jpeg::JpegEncoder::new_with_quality(
            std::fs::File::create(&path).unwrap(),
            90,
        )
        .encode_image(&img)
        .unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        let req = thumb_request(path.to_string_lossy().as_ref(), 1, size, 64);
        let reply = handle_ctrl_request_reply(state, req).await;
        let (ok, jpeg) = thumb_jpeg(&reply).expect("thumb 恒单帧 Thumb 形");
        assert!(ok, "thumb 恒成功帧(失败=空字节,不回错误帧)");
        assert!(!jpeg.is_empty(), "图片出非空 jpeg");
        let decoded = image::load_from_memory(&jpeg).expect("JPEG 可解");
        assert!(decoded.width() <= 64 && decoded.height() <= 64);
    }

    /// T7 分发缝(无后端类型,全平台):.txt → ok=true + **空字节**单帧
    /// (失败=空字节,不回错误帧;brief 钉死契约)。
    #[tokio::test]
    async fn thumb_reply_unsupported_type_is_ok_empty_frame() {
        let state = test_state();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, b"plain").unwrap();
        let req = thumb_request(path.to_string_lossy().as_ref(), 1, 5, 64);
        let reply = handle_ctrl_request_reply(state, req).await;
        let (ok, jpeg) = thumb_jpeg(&reply).expect("thumb 恒单帧 Thumb 形");
        assert!(ok, "thumb 恒成功帧");
        assert!(jpeg.is_empty(), "无后端类型 = 空字节");
    }

    /// T7 守卫(分发缝):集外路径 Thumb INVALID_PATH 单帧拒(守卫先于
    /// 引擎,不落生成面)。
    #[tokio::test]
    async fn thumb_reply_outside_roots_rejected() {
        let (state, _home) = test_state_with_home();
        std::fs::create_dir_all(_home.path().join("docs")).unwrap();
        std::fs::create_dir_all(_home.path().join("secret")).unwrap();
        std::fs::write(_home.path().join("secret").join("s.txt"), b"s").unwrap();
        state.set_roots(vec![RootSpec {
            name: "docs".into(),
            path: _home.path().join("docs").to_string_lossy().into_owned(),
            source: RootSource::Fs,
            visible: true,
        }]);
        let req = thumb_request(
            _home
                .path()
                .join("secret")
                .join("s.txt")
                .to_string_lossy()
                .as_ref(),
            0,
            1,
            64,
        );
        let reply = handle_ctrl_request_reply(state, req).await;
        let got = frame_code(&reply);
        assert_eq!(
            got,
            Some((false, "INVALID_PATH".into())),
            "集外 Thumb INVALID_PATH: {got:?}"
        );
    }

    /// T7 服务桥 vpath 直通臂(T1-M2 转达):守卫对 bridge vpath 放行
    /// (对齐 FileReadRange 口径)→ 走导出导轨;App 缺席 = 导出不可达
    /// → **空字节 ok 帧**(预览降级语义,不回 INVALID_PATH/错误帧)。
    /// 本用例同时是直通臂的证词:无直通臂时守卫按集外拒(INVALID_PATH),
    /// 与空字节 ok 帧可区分。
    #[tokio::test]
    async fn thumb_reply_bridge_vpath_passes_guard_routes_export_rail() {
        let (state, _home) = test_state_with_home();
        let root = _home.path().join("桥根");
        std::fs::create_dir_all(&root).unwrap();
        state.set_roots(vec![RootSpec {
            name: "桥".into(),
            path: root.to_string_lossy().into_owned(),
            source: RootSource::Bridge,
            visible: true,
        }]);
        let vpath = format!("{}{}photo.jpg", root.display(), std::path::MAIN_SEPARATOR);
        let req = thumb_request(&vpath, 1, 100, 64);
        let reply = handle_ctrl_request_reply(state, req).await;
        let (ok, jpeg) = thumb_jpeg(&reply).expect("thumb 恒单帧 Thumb 形");
        assert!(ok, "桥导出失败仍恒成功帧(空字节降级)");
        assert!(jpeg.is_empty(), "App 缺席 = 导出不可达 = 空字节");
    }

    /// T9 照片臂守卫(photo_serve 关):photo vpath Thumb 落既有守卫按集外
    /// 拒(INVALID_PATH;行为与泛化前逐字节一致——guard.check 直臂;unix
    /// 门:photo_vpath 词法规范化按平台绝对形态收口,photo_vpath_matrix 同门)。
    #[cfg(unix)]
    #[tokio::test]
    async fn thumb_reply_photo_vpath_photo_off_rejected_by_guard() {
        let (state, _home) = test_state_with_home();
        std::fs::create_dir_all(_home.path().join("docs")).unwrap();
        state.set_roots(vec![RootSpec {
            name: "docs".into(),
            path: _home.path().join("docs").to_string_lossy().into_owned(),
            source: RootSource::Fs,
            visible: true,
        }]);
        let req = thumb_request("/照片图库/相册/x.jpg", 0, 1, 64);
        let reply = handle_ctrl_request_reply(state, req).await;
        let got = frame_code(&reply);
        assert_eq!(
            got,
            Some((false, "INVALID_PATH".into())),
            "photo_serve 关 = 既有集外拒(行为不变): {got:?}"
        );
    }

    /// T9 照片臂路由(photo_serve 开):守卫照片臂放行(T8 Parked #2 挂入)
    /// → 桥先行(无订阅快败)→ 照片导出件回退(单查快败)→ **空字节 ok
    /// 帧**(恒成功形;泛化前 = INVALID_PATH 错误帧,本用例同时是照片臂的
    /// 证词;unix 门同上)。
    #[cfg(unix)]
    #[tokio::test]
    async fn thumb_reply_photo_vpath_routes_bridge_then_photo_fallback() {
        let (events, _) = tokio::sync::broadcast::channel(16);
        let _home = tempfile::tempdir().unwrap();
        let state = {
            let session = crate::session::Session {
                server: "http://127.0.0.1:9".into(),
                jwt: "test".into(),
                refresh: "test".into(),
                device_id: "selfdev".into(),
                ca: None,
            };
            session.save(_home.path()).unwrap();
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
                    _home.path(),
                    _home.path().join("inbox"),
                    None,
                    inbox_dest::staging_dir(_home.path()),
                    false,
                    true, // photo_serve 开(照片桥随 state 常驻)
                    Default::default(),
                    Default::default(),
                    crate::ctrl::ctrl_default_tuning(),
                    events,
                    update,
                )
                .unwrap(),
            )
        };
        let req = thumb_request("/照片图库/相册/x.jpg", 0, 0, 64);
        let reply = handle_ctrl_request_reply(state, req).await;
        let (ok, jpeg) = thumb_jpeg(&reply).expect("thumb 恒单帧 Thumb 形");
        assert!(ok, "照片臂仍恒成功帧(桥快败 + 回退快败 = 空字节降级)");
        assert!(jpeg.is_empty(), "无订阅快败 → 回退快败 = 空字节");
    }
}
