//! 缩略图启动预热(field r26,用户架构提案落地):daemon 就绪后后台
//! 低优先级扫源端可缩略图面(授权根/收件箱的图片·视频文件 + 照片图库
//! vpath),逐项经 [`super::thumb::thumb_routed`](与在线请求同一路由:
//! 服务桥/照片桥/平台后端)生成并**落地** `{home}/thumb-tmp` 512 MiB
//! LRU——冷缓存首览逐个现算的「慢+卡」根因由此消解:预热完成后任何
//! 终端浏览即读盘,源端零现算。
//!
//! 设计口径:
//! - **节流**:串行逐项 + 项间 80ms 呼吸;总预算
//!   [`PREWARM_BUDGET`](默认 10 分钟)到点即止(未扫完的留待下次启动
//!   ——缓存只增,断点续扫天然成立);单目录列举/读档失败跳过不重试。
//! - **键宽**:统一 w=[`PREWARM_W`](256,与查看端最常见请求面一致:
//!   面板封面/平铺 m 档/移动浏览行);其他宽度的首次请求仍按需生成
//!   (LRU 常驻后渐热)。
//! - **关停**:`MTDSS_THUMB_PREWARM=0` 环境变量禁用(测试/低电场景)。
//! - **桌面默认面**(守卫未启用):只扫 `{home}/inbox`(整机漫游不近
//!   情);移动面(守卫启用)扫全部 fs 授权根 + 照片图库(桥列举递归)。
//! - 照片图库经 [`crate::photo::PhotoBridge::list`] 递归(桥不可达/
//!   App 未连即整臂跳过——预热是尽力而为面,不重试不告警)。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::state::DaemonState;

/// 预热键宽(查看端最常见请求面:面板封面/平铺 m 档/移动浏览行)。
const PREWARM_W: u32 = 256;
/// 单项生成后的呼吸间隔(串行 + 间隔 = 源端无感:不抢 App 主线程/磁盘)。
const PREWARM_GAP: Duration = Duration::from_millis(80);
/// 每轮批大小(field r26b:一轮 N 项 → 睡一轮;串行恒定,无并发)。
const PREWARM_BATCH: usize = 8;
/// 轮间隔(批间长睡:8 项/30s ≈ 960 项/时 ≈ 2.3 万项/日——大库也从容)。
const PREWARM_ROUND: Duration = Duration::from_secs(30);
/// 全量扫完后进入增量巡检:每 10 分钟重收集一遍(新文件/mtime 变化的
/// 键自然 miss 重生成;列举本身只 readdir,无解码成本)。
const PREWARM_RESCAN: Duration = Duration::from_secs(10 * 60);
/// 任务总窗(field r26b 用户裁定 24 小时制):自 daemon 启动计时,
/// 窗内持续定时增量;到点收工,下次 daemon 启动重新起窗。daemon 进程
/// 退出 = 任务随 runtime 终结(无独立线程)。
const PREWARM_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);
/// 每生成 N 项打一条进度(可观测面)。
const PREWARM_LOG_EVERY: u32 = 50;

/// 缩略图候选扩展名(image 面 + 视频 face;与 thumb.rs 后端词表同源——
/// 生成端天然按内容分型,这里只做廉价的要不要试筛)。
fn is_candidate(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "heic" | "tif" | "tiff" | "ico" | "avif"
            | "mp4" | "mkv" | "mov" | "avi" | "m4v" | "wmv" | "flv" | "mpg" | "mpeg" | "3gp" | "ts"
    )
}

/// 预热任务候选(路径 + 身份键原料)。
struct Candidate {
    path: String,
    mtime: i64,
    size: u64,
}

/// 启动预热(尽力而为;调用方 spawn 后不等待不观测——失败只留 debug 日志)。
pub(crate) fn spawn(state: Arc<DaemonState>) {
    if std::env::var("MTDSS_THUMB_PREWARM").ok().as_deref() == Some("0") {
        tracing::info!("缩略图预热已禁用(MTDSS_THUMB_PREWARM=0)");
        return;
    }
    tokio::spawn(async move {
        run(state).await;
    });
}

/// 主环(field r26b 24 小时制):窗内「批推进 → 轮睡」往复;全量扫完
/// 转 10 分钟增量巡检(重收集,新/变更键 miss 重生成);窗到点收工。
/// daemon 退出 = runtime 收摊 = 本任务自然终结。
async fn run(state: Arc<DaemonState>) {
    let started = std::time::Instant::now();
    let mut done: u32 = 0;
    let mut stored: u32 = 0;
    let mut walk = match collect_candidates(&state).await {
        Ok(w) => w,
        Err(e) => {
            tracing::debug!(err = %e, "缩略图预热:候选收集失败(跳过本轮)");
            return;
        }
    };
    tracing::info!(
        total = walk.len(),
        batch = PREWARM_BATCH,
        round = ?PREWARM_ROUND,
        window = ?PREWARM_WINDOW,
        "缩略图预热开始(24h 定时增量,后台静默)"
    );
    loop {
        if started.elapsed() >= PREWARM_WINDOW {
            tracing::info!(done, stored, "缩略图预热 24h 窗到点收工(下次启动重起窗)");
            return;
        }
        // 一轮批(串行逐项 + 项间呼吸;批满或队空即止)
        let mut took = 0usize;
        while took < PREWARM_BATCH {
            let Some(c) = walk.pop() else { break };
            let jpeg = super::thumb::thumb_routed(&state, &c.path, c.mtime, c.size, PREWARM_W).await;
            if !jpeg.is_empty() {
                stored += 1;
            }
            done += 1;
            took += 1;
            if done % PREWARM_LOG_EVERY == 0 {
                tracing::debug!(done, stored, remain = walk.len() as u32, "缩略图预热进度");
            }
            tokio::time::sleep(PREWARM_GAP).await;
        }
        if walk.is_empty() {
            // 全量到尾:转增量巡检(睡长档再重收集;新文件/变更键自然接上)
            tracing::debug!(done, stored, "缩略图预热全量扫完,转增量巡检(每 {:?})", PREWARM_RESCAN);
            tokio::time::sleep(PREWARM_RESCAN).await;
            match collect_candidates(&state).await {
                Ok(w) => walk = w,
                Err(_) => continue, // 桥暂不可达等:下一巡检窗再试
            }
        } else {
            tokio::time::sleep(PREWARM_ROUND).await;
        }
    }
}

/// 候选收集:fs 根递归 + 照片图库桥递归。
async fn collect_candidates(state: &Arc<DaemonState>) -> Result<Vec<Candidate>, String> {
    let mut out: Vec<Candidate> = Vec::new();
    // fs 面:守卫启用 = 全部 fs 授权根;未启用(桌面默认)= 仅收件箱。
    let guard = state.guard();
    let mut fs_roots: Vec<PathBuf> = Vec::new();
    if guard.enabled() {
        for entry in guard.iter_fs_roots() {
            fs_roots.push(entry);
        }
    }
    fs_roots.push(state.recv_dir());
    for root in fs_roots {
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let rd = match tokio::task::spawn_blocking({
                let d = dir.clone();
                move || std::fs::read_dir(&d)
            })
            .await
            .map_err(|e| e.to_string())?
            {
                Ok(r) => r,
                Err(_) => continue, // 不可读目录跳过(授权失效/权限/消失)
            };
            for entry in rd.flatten() {
                let Ok(ft) = entry.file_type() else { continue };
                if ft.is_dir() {
                    stack.push(entry.path());
                } else if ft.is_file() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if !is_candidate(&name) {
                        continue;
                    }
                    let Ok(md) = entry.metadata() else { continue };
                    out.push(Candidate {
                        path: entry.path().to_string_lossy().into_owned(),
                        mtime: md
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0),
                        size: md.len(),
                    });
                }
            }
        }
    }
    // 照片图库面(桥在场才扫;App 未连 = 整臂跳过)。
    if let Some(bridge) = state.photo() {
        let mut stack: Vec<String> = vec![String::new()];
        while let Some(sub) = stack.pop() {
            let Ok((entries, _truncated)) = bridge.list(&sub).await else {
                continue;
            };
            for e in entries {
                if e.is_dir {
                    stack.push(crate::photo::join_vpath(&sub, &e.name));
                } else if is_candidate(&e.name) {
                    out.push(Candidate {
                        path: crate::photo::join_vpath(&sub, &e.name),
                        mtime: e.mtime_ms as i64,
                        size: e.size,
                    });
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_ext_matrix() {
        assert!(is_candidate("a.jpg"));
        assert!(is_candidate("B.HEIC"));
        assert!(is_candidate("c.mp4"));
        assert!(is_candidate("d.mkv"));
        assert!(!is_candidate("e.txt"));
        assert!(!is_candidate("f.pdf"));
        assert!(!is_candidate("无扩展名"));
    }
}

#[cfg(test)]
mod prewarm_tests {
    use super::*;
    use crate::state::DaemonState;

    /// 预热全链:fs 收件目录两张 PNG → run() → 源端缓存落 2 条
    /// (thumb_routed 本机臂 → image 后端非 Windows;Windows 用 Shell
    /// COM 同链)。预算压缩防测试拖时。
    #[cfg(not(windows))]
    #[tokio::test]
    async fn prewarm_walks_recv_dir_and_populates_cache() {
        let home = tempfile::tempdir().unwrap();
        let inbox = home.path().join("inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        let img = image::RgbImage::from_pixel(8, 8, image::Rgb([10, 200, 30]));
        for name in ["a.png", "b.png"] {
            image::DynamicImage::ImageRgb8(img.clone())
                .save_with_format(inbox.join(name), image::ImageFormat::Png)
                .unwrap();
        }
        std::fs::write(inbox.join("skip.txt"), b"not a thumb").unwrap();
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
                inbox.clone(),
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
        // 单趟直驱(r26b 起主环是 24h 常驻,测试不进主环:collect +
        // thumb_routed 同链一趟,断言与主环逐项面等价)
        for c in collect_candidates(&state).await.unwrap() {
            let _ = crate::preview::thumb::thumb_routed(&state, &c.path, c.mtime, c.size, PREWARM_W).await;
        }
        assert_eq!(state.preview_thumb_cache().len(), 2, "两张 PNG 落缓存,txt 不落");
    }
}
