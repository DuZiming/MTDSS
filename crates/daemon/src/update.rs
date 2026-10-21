#![cfg(windows)]
//! daemon 更新核心(M4b Task 11/12):状态机/检查/下载/验签/空闲门/
//! Installing 序列(标记 + 原子契约 + detached spawn)与开机标记消费。
//!
//! 信任模型:清单为 canonical ed25519 签名(core::update::UpdateManifest),
//! 验签公钥是唯一信任锚;验签通过后清单内字段(含资产 URL)即为受信,
//! 不另设 URL 白名单。下载落 `{home}/downloads/mtdss-setup-<ver>.exe`,
//! 临时名 `.part`,SHA-256 与字节数复核通过后才 rename 就位。
//!
//! fail-closed 口径(错误分类三分):
//! - **未配置拒绝**(公钥 None):check/apply 一律 Err「更新未配置」,状态
//!   不翻——后台循环同样静默不查不装(否则无钥构建每次启动都翻 failed);
//! - **操作失败**(清单拉取/解析/验签/版本非法/下载/复核/rename/标记写
//!   失败):一律 state=failed + `event.update` 广播 + 临时文件即删,系统
//!   零改动(标记未写 = 未进入 Installing,可重试);
//! - **空闲门拒绝**(有传输作业,含已暂停):code 1 + `data.kind=
//!   JOBS_ACTIVE` + 文案精确,转 pending_install(非 failed),作业终态
//!   事件或下轮定时重试([`UpdateService::run`])。
//!
//! 状态机:`idle → available → downloading → ready → installing`(Task 12:
//! apply 在下载复核成功后接 Installing 序列,ready 是序列内的短暂翻转);
//! 分支 failed / pending_install。下载/挂起/就绪/安装期 check 不改写状态
//! (404/网络错误不得打翻进行中的下载、已就绪的包或正在收尾的安装)。
//! 钉扎语义:定时检查拉 `{base}/win/{current}/manifest.json`,404 = 该版本
//! 无更新(常态)→ idle 而非失败;force 检查(RPC update.check)拉
//! `{base}/manifest.json`。apply(显式 version)同走钉扎 URL,M4 无防回滚。
//!
//! Installing 序列(Task 12,见 [`UpdateService::installing_sequence`]):
//! 作业快照冲刷 → 写 restart-ui(仅当有已连客户端)→ 原子写 update-
//! in-progress(内容 = 目标版本)→ 广播 installing 事件 → 等已连客户端
//! 归零(≤5s,超时不候)→ detached spawn 已验签 setup(`/S`;测试注入
//! 桩)→ 发退出请求(与托盘「退出」同一条 watch,main 汇入
//! `Daemon::shutdown` 全套收尾)——spawn 与退出请求间零等待。开机消费
//! ([`consume_boot_markers`]):等版本 → 清标记 + 「已更新到 X」通知;
//! 不等 → 未完成告警 + 清除;restart-ui 在场 → 清除 + 唤回主窗。
//!
//! 事件契约:`event.update` 帧 = 状态 JSON 同形(state/current/latest/notes/
//! progress/error);状态翻转即发,进度帧沿 jobs 的 [`PROGRESS_EVENT_MIN_GAP`]
//! (≥400ms)节流:首条即发,状态翻转帧不受限。
//!
//! 平台门:M4 更新仅 Windows——本模块 `#![cfg(windows)]` 整体门住;api.rs
//! 的 update.* 臂在非 Windows 回 UNSUPPORTED_PLATFORM(薄垫片),state/lib
//! 以单元占位别名装配,非 Windows 构建不进本模块依赖图。

use crate::jobs::{lock_ok, PROGRESS_EVENT_MIN_GAP};
use crate::state::DaemonState;
use ed25519_dalek::VerifyingKey;
use mtdss_core::update::{version_cmp, PlatformAsset, UpdateManifest};
use mtdss_ipc::{EventFrame, RpcError};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;

/// 事件方法名:更新状态翻转(即发)与节流进度(≥400ms)。
pub(crate) const EV_UPDATE: &str = "event.update";

/// 业务失败错误码(与 api.rs ERR_BUSINESS 同码位;模块内自持免跨引)。
const ERR_BUSINESS: i64 = 1;
/// 参数非法错误码(update.apply 的 version 白名单校验)。
const ERR_INVALID_PARAMS: i64 = -32602;
/// 清单平台键(M4 仅 Windows;core::update 清单的键形态)。
const PLATFORM_KEY: &str = "windows-x86_64";
/// 清单请求超时(小 JSON;与登录 REST 面同口径)。
const CHECK_TIMEOUT: Duration = Duration::from_secs(15);
/// 下载请求超时(整包上界;远大于典型安装包传输时长)。
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
/// 连接建立超时(挂起连接不占调用面)。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 自动检查周期(D7:24h;tokio interval 首拍即发 = 启动即查)。
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// Installing 序列等已连客户端归零的上界(超时不候:安装器必须走,
/// 不能被一个挂死的客户端拖住)。
const INSTALL_DRAIN: Duration = Duration::from_secs(5);
/// 归零轮询间隔。
const INSTALL_DRAIN_POLL: Duration = Duration::from_millis(250);

/// 更新服务配置(base/CA/验签公钥/当前版本/installer spawn 注入面)。
/// `pub` 仅为集成测试注入面(`Daemon::start_with_update` 携带;与 flow/ops
/// 的 pub 模块同款口径),其余构造面(UpdateService、run)保持 pub(crate)。
#[derive(Clone)]
pub struct UpdateConfig {
    /// 更新通道基址(拼 `/manifest.json` 与 `/win/{ver}/manifest.json`)。
    pub base_url: String,
    /// 自签 https 场景的 CA 证书路径;None 用默认 webpki 根。
    pub ca: Option<std::path::PathBuf>,
    /// 清单验签公钥;None = 更新未配置(check/apply 一律拒绝,后台静默)。
    pub pubkey: Option<VerifyingKey>,
    /// 当前守护进程版本(严格 X.Y.Z;钉扎 URL 与新旧判定的基准)。
    pub current: String,
    /// 安装器 spawn 注入面(Task 12):Some 时 Installing 序列以它代替真实
    /// detached Command(实参 = 已验签 setup 路径);生产 None → 真实
    /// Command(`/S` 静默安装,DETACHED_PROCESS|CREATE_NO_WINDOW)。
    pub spawner: Option<Arc<dyn Fn(PathBuf) + Send + Sync>>,
}

impl Default for UpdateConfig {
    /// 失败安全缺省:`.invalid` 占位基址(永不解析)+ 无公钥(未配置,
    /// 一切 check/apply 拒绝)+ 无注入桩(生产 spawn 面)——即使被误用也
    /// 不可能触达真实端点或放行更新。
    fn default() -> Self {
        Self {
            base_url: "https://mtdss-update.invalid".into(),
            ca: None,
            pubkey: None,
            current: env!("CARGO_PKG_VERSION").into(),
            spawner: None,
        }
    }
}

/// 构建期注入的公钥 hex → 验签钥;非 hex / 长度错 → None(调用方记 warn)。
fn parse_pubkey(h: &str) -> Option<VerifyingKey> {
    let bytes = hex::decode(h).ok()?;
    let arr: [u8; 32] = bytes.try_into().ok()?;
    VerifyingKey::from_bytes(&arr).ok()
}

impl UpdateConfig {
    /// 生产配置:base = `mtdss_ipc::consts::update_url()`(运行时 env
    /// MTDSS_UPDATE_URL → 构建期注入 → .invalid 占位),CA = 运行时 env
    /// MTDSS_UPDATE_CA,公钥 = 构建期 MTDSS_UPDATE_PUBKEY hex(缺失/空/
    /// 非法 → None → 更新未配置,fail-closed),当前版本 = 本 crate 版本。
    pub fn production() -> Self {
        let pubkey = option_env!("MTDSS_UPDATE_PUBKEY")
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .and_then(|h| {
                let vk = parse_pubkey(h);
                if vk.is_none() {
                    tracing::warn!("MTDSS_UPDATE_PUBKEY 非法(非 hex/长度错),更新验证未配置(fail-closed)");
                }
                vk
            });
        Self {
            base_url: mtdss_ipc::consts::update_url(),
            ca: std::env::var("MTDSS_UPDATE_CA")
                .ok()
                .map(std::path::PathBuf::from),
            pubkey,
            current: env!("CARGO_PKG_VERSION").into(),
            spawner: None,
        }
    }
}

/// 更新状态机的可变面(锁内持改;全部操作毫秒级,无长阻塞)。std Mutex:
/// 锁内无 await;毒化经 [`lock_ok`] 恢复(与 jobs 同款)。
struct UpdateSt {
    /// idle|available|downloading|ready|pending_install|installing|failed
    /// (installing 属 Task 12:Installing 序列内的终段翻转,daemon 随后
    /// 收尾退出)。
    state: &'static str,
    latest: Option<String>,
    notes: Option<String>,
    /// 0.0-1.0(仅 downloading 期间有意义;终态复位 null)。
    progress: Option<f64>,
    error: Option<String>,
    /// 最近一次验签通过的清单缓存:apply(None) 与 pending_install 重试的
    /// 数据源(失败后保留——清单仍验签有效,重试无需重新 check)。
    manifest: Option<UpdateManifest>,
    /// 进度节流:上一条进度帧时刻(首条 None → 即发)。
    last_progress_emit: Option<Instant>,
}

/// daemon 更新服务:配置 + 状态机 + 事件发送端 + apply 串行化。
/// 由 DaemonState 持 Arc(生产)/ 测试经 `Daemon::start_with_update` 注入。
pub(crate) struct UpdateService {
    cfg: UpdateConfig,
    st: Mutex<UpdateSt>,
    events: tokio::sync::broadcast::Sender<EventFrame>,
    /// apply/下载串行化:并发第二请求在锁上排队,醒来看到 ready 即幂等返回。
    apply_lock: AsyncMutex<()>,
    /// 退出请求(与托盘「退出」同一条 watch):Installing 序列 spawn 完
    /// 安装器后置位,main 在两条源上任一翻转即走 `Daemon::shutdown` 全套
    /// 收尾(--no-tray 且无接收方时置位被忽略,安装序列照常完成)。
    quit: tokio::sync::watch::Sender<bool>,
}

impl UpdateService {
    pub(crate) fn new(
        cfg: UpdateConfig,
        events: tokio::sync::broadcast::Sender<EventFrame>,
        quit: tokio::sync::watch::Sender<bool>,
    ) -> Self {
        Self {
            cfg,
            st: Mutex::new(UpdateSt {
                state: "idle",
                latest: None,
                notes: None,
                progress: None,
                error: None,
                manifest: None,
                last_progress_emit: None,
            }),
            events,
            apply_lock: AsyncMutex::new(()),
            quit,
        }
    }

    // ---------- 状态投影 ----------

    /// 状态 JSON(事件与 update.state 同形;字段面钉死)。
    fn state_json(&self, st: &UpdateSt) -> serde_json::Value {
        serde_json::json!({
            "state": st.state,
            "current": self.cfg.current,
            "latest": st.latest,
            "notes": st.notes,
            "progress": st.progress,
            "error": st.error,
        })
    }

    /// 当前状态 JSON(update.state RPC 数据源)。
    pub(crate) fn state(&self) -> serde_json::Value {
        let st = lock_ok(&self.st);
        self.state_json(&st)
    }

    fn current_state(&self) -> &'static str {
        lock_ok(&self.st).state
    }

    /// 状态翻转帧广播(调用方持锁;仅状态真正变化时调用)。
    fn emit(&self, st: &UpdateSt) {
        let _ = self.events.send(EventFrame {
            method: EV_UPDATE.into(),
            params: self.state_json(st),
        });
    }

    // ---------- 落定面(各终/中间态;同态幂等不重发) ----------

    fn settle_idle(&self) {
        let mut st = lock_ok(&self.st);
        if st.state == "idle" {
            return;
        }
        st.state = "idle";
        st.latest = None;
        st.notes = None;
        st.progress = None;
        st.error = None;
        st.manifest = None;
        st.last_progress_emit = None;
        self.emit(&st);
    }

    fn settle_available(&self, m: UpdateManifest) {
        let mut st = lock_ok(&self.st);
        if st.state == "available" && st.latest.as_deref() == Some(m.version.as_str()) {
            st.manifest = Some(m); // 同版本幂等:仅刷新缓存,不重发事件
            return;
        }
        st.state = "available";
        st.latest = Some(m.version.clone());
        st.notes = Some(m.notes.clone());
        st.progress = None;
        st.error = None;
        st.manifest = Some(m);
        st.last_progress_emit = None;
        self.emit(&st);
    }

    fn settle_pending_install(&self, m: &UpdateManifest) {
        let mut st = lock_ok(&self.st);
        let changed = st.state != "pending_install";
        st.state = "pending_install";
        st.latest = Some(m.version.clone());
        st.notes = Some(m.notes.clone());
        st.progress = None;
        st.error = None;
        st.manifest = Some(m.clone());
        st.last_progress_emit = None;
        if changed {
            self.emit(&st);
        }
    }

    fn settle_downloading(&self, m: &UpdateManifest) {
        let mut st = lock_ok(&self.st);
        st.state = "downloading";
        st.latest = Some(m.version.clone());
        st.notes = Some(m.notes.clone());
        // progress 保持 null:进度只来自真实字节记账(track_progress),
        // 翻转帧不伪造 0.0(否则紧贴首条进度帧,节流口径上出现假近邻)
        st.progress = None;
        st.error = None;
        st.manifest = Some(m.clone());
        st.last_progress_emit = None; // 首条进度帧即发(沿 jobs 口径)
        self.emit(&st);
    }

    fn settle_ready(&self, m: &UpdateManifest) {
        let mut st = lock_ok(&self.st);
        st.state = "ready";
        st.latest = Some(m.version.clone());
        st.notes = Some(m.notes.clone());
        st.progress = None;
        st.error = None;
        st.manifest = Some(m.clone());
        st.last_progress_emit = None;
        self.emit(&st);
    }

    /// Installing 终段翻转(Task 12):Installing 序列的标记已落盘后广播;
    /// daemon 随安装器 spawn 与退出请求收尾,状态不再离开 installing。
    fn settle_installing(&self, m: &UpdateManifest) {
        let mut st = lock_ok(&self.st);
        st.state = "installing";
        st.latest = Some(m.version.clone());
        st.notes = Some(m.notes.clone());
        st.progress = None;
        st.error = None;
        st.manifest = Some(m.clone());
        st.last_progress_emit = None;
        self.emit(&st);
    }

    /// 操作失败落定:state=failed + 事件广播;返回 RPC 业务错(系统零改动
    /// 由调用方保证:临时文件已在调用侧删除)。
    fn fail(&self, msg: &str) -> RpcError {
        {
            let mut st = lock_ok(&self.st);
            st.state = "failed";
            st.error = Some(msg.to_string());
            st.progress = None;
            st.last_progress_emit = None;
            // manifest 缓存保留:清单仍验签有效,失败后 apply(None) 可直接重试
            self.emit(&st);
        }
        tracing::warn!("更新失败: {msg}");
        RpcError::new(ERR_BUSINESS, msg)
    }

    /// 下载进度:锁内更新比例 + 节流广播(首条即发,其后 ≥400ms)。
    /// 终态后迟到的回调静默丢弃(与 jobs track_common 同口径)。
    fn track_progress(&self, done: u64, total: u64) {
        let mut st = lock_ok(&self.st);
        if st.state != "downloading" {
            return;
        }
        let frac = if total > 0 {
            ((done as f64) / (total as f64)).min(1.0)
        } else {
            0.0
        };
        st.progress = Some(frac);
        let due = st
            .last_progress_emit
            .map(|t| t.elapsed() >= PROGRESS_EVENT_MIN_GAP)
            .unwrap_or(true);
        if due {
            st.last_progress_emit = Some(Instant::now());
            self.emit(&st);
        }
    }

    // ---------- 检查 ----------

    /// 检查更新:force 拉 `{base}/manifest.json`,否则钉扎
    /// `{base}/win/{current}/manifest.json`(404 = 无更新 → idle)。
    /// 验签失败/版本非法 → failed + Err;较新 → available + 事件;旧/等 →
    /// idle。下载/挂起/就绪期间不改写状态(进行中的下载不得被网络错误打翻)。
    pub(crate) async fn check(&self, force: bool) -> Result<serde_json::Value, RpcError> {
        // 未配置:拒绝但不翻状态(配置缺失不是操作失败;后台循环同样静默)
        let Some(vk) = self.cfg.pubkey.as_ref() else {
            return Err(RpcError::new(ERR_BUSINESS, "更新未配置(缺少验签公钥)"));
        };
        if matches!(
            self.current_state(),
            "downloading" | "pending_install" | "ready" | "installing"
        ) {
            return Ok(self.state());
        }
        let url = if force {
            format!("{}/manifest.json", self.cfg.base_url.trim_end_matches('/'))
        } else {
            format!(
                "{}/win/{}/manifest.json",
                self.cfg.base_url.trim_end_matches('/'),
                self.cfg.current
            )
        };
        let client = self.http_client().map_err(|m| self.fail(&m))?;
        let resp = client
            .get(&url)
            .timeout(CHECK_TIMEOUT)
            .send()
            .await
            .map_err(|e| self.fail(&format!("拉取更新清单失败: {e}")))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            self.settle_idle();
            return Ok(self.state());
        }
        if !resp.status().is_success() {
            return Err(self.fail(&format!("拉取更新清单失败: HTTP {}", resp.status())));
        }
        let manifest: UpdateManifest = resp
            .json()
            .await
            .map_err(|e| self.fail(&format!("解析更新清单失败: {e}")))?;
        // 先验签(唯一信任锚)再比版本;非法版本按无效清单拒绝
        if !manifest.verify(vk) {
            return Err(self.fail("清单验签失败"));
        }
        let cmp = version_cmp(&manifest.version, &self.cfg.current)
            .ok_or_else(|| self.fail("清单版本非法(须为严格 X.Y.Z)"))?;
        match cmp {
            Ordering::Greater => self.settle_available(manifest),
            _ => self.settle_idle(),
        }
        Ok(self.state())
    }

    // ---------- 应用(下载复核 → Installing 序列收尾) ----------

    /// 应用更新:idle 门(有传输作业[含已暂停] → JOBS_ACTIVE 拒绝并转
    /// pending_install)→ downloading(节流进度)→ SHA-256/字节数复核 →
    /// ready → Installing 序列([`UpdateService::installing_sequence`],标
    /// 记/installing 事件/等客户端归零/detached spawn/退出请求)。version
    /// 缺省用已检得的清单缓存;显式 version 走钉扎 URL(M4 无防回滚,降级
    /// 被接受)。ready/installing 幂等返回;并发 apply 经锁串行。
    pub(crate) async fn apply(
        &self,
        version: Option<String>,
        state: &Arc<DaemonState>,
    ) -> Result<serde_json::Value, RpcError> {
        // Option<VerifyingKey> 是 Copy:直接拷出,锁外验签
        let Some(vk) = self.cfg.pubkey else {
            return Err(RpcError::new(ERR_BUSINESS, "更新未配置(缺少验签公钥)"));
        };
        let _permit = self.apply_lock.lock().await;
        if matches!(self.current_state(), "ready" | "installing") {
            // ready:前一 apply 已下载就绪(理论态——ready 在序列内即刻转
            // installing);installing:安装序列已收尾,不得二次安装。
            return Ok(self.state());
        }
        let manifest = match version {
            None => match lock_ok(&self.st).manifest.clone() {
                Some(m) => m,
                None => return Err(self.fail("没有可安装的更新,请先 update.check")),
            },
            Some(v) => self.fetch_pinned_manifest(&v, &vk).await?,
        };
        let Some(asset) = manifest.platforms.get(PLATFORM_KEY) else {
            return Err(self.fail(&format!("清单缺少 {PLATFORM_KEY} 资产")));
        };
        // 空闲门:有传输(含已暂停)→ 拒绝 + pending_install(非 failed),
        // 作业终态事件或下轮定时重试
        let active = state.jobs().active_count();
        if active > 0 {
            self.settle_pending_install(&manifest);
            return Err(RpcError::with_data(
                ERR_BUSINESS,
                format!("{active} 个传输中(含已暂停),可先取消作业后重试"),
                serde_json::json!({ "kind": "JOBS_ACTIVE" }),
            ));
        }
        self.settle_downloading(&manifest);
        let dir = state.home().join("downloads");
        let ver = &manifest.version;
        let final_path = dir.join(format!("mtdss-setup-{ver}.exe"));
        let tmp_path = dir.join(format!("mtdss-setup-{ver}.exe.part"));
        let outcome: Result<(), String> = async {
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(|e| format!("创建下载目录失败: {e}"))?;
            self.download_asset(asset, &tmp_path).await?;
            tokio::fs::rename(&tmp_path, &final_path)
                .await
                .map_err(|e| format!("下载就位失败(rename): {e}"))?;
            Ok(())
        }
        .await;
        if let Err(msg) = outcome {
            // 失败即删临时件:系统零改动(下载文件不算系统改动)
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(self.fail(&msg));
        }
        self.settle_ready(&manifest);
        // Installing 序列(Task 12):标记/事件/等待/spawn/退出请求。标记写
        // 失败 → failed(标记未写 = 未进入 Installing,可重试)。
        self.installing_sequence(&manifest, state, &final_path).await?;
        Ok(self.state())
    }

    // ---------- Installing 序列(Task 12) ----------

    /// apply 下载复核成功后的安装契约收尾,依次:
    /// ① 作业快照冲刷(注册表内每作业广播一帧现状/终态,UI 卡片在退出前
    ///    拿全量);
    /// ② 写 restart-ui 标记(仅当有已连客户端——更新退出会关掉 UI,新
    ///    daemon 开机消费时唤回主窗);
    /// ③ 原子写 update-in-progress(内容 = 目标版本)。**失败 → failed:
    ///    标记未写 = 未进入 Installing,可重试**;已写的 restart-ui 一并
    ///    回删,不留半套标记;
    /// ④ 广播 installing 事件(状态机 ready → installing);
    /// ⑤ 等已连客户端归零(≤5s,250ms 轮询;超时不候);
    /// ⑥ detached spawn 已验签 setup(`/S`;测试注入桩,生产真实 Command);
    /// ⑦ 发退出请求(与托盘「退出」同一条 watch;main 汇入
    ///    `Daemon::shutdown` 全套收尾)——spawn 与退出请求间零等待。
    async fn installing_sequence(
        &self,
        m: &UpdateManifest,
        state: &Arc<DaemonState>,
        setup: &Path,
    ) -> Result<(), RpcError> {
        self.flush_job_frames(state);
        let home = state.home();
        let restart_marker = mtdss_ipc::restart_ui_marker_path(&home);
        let connected = mtdss_ipc::connected_clients() > 0;
        if connected {
            crate::session::write_atomic(&restart_marker, "")
                .map_err(|e| self.fail(&format!("写重启标记失败: {e}")))?;
        }
        let update_marker = mtdss_ipc::update_marker_path(&home);
        if let Err(e) = crate::session::write_atomic(&update_marker, &m.version) {
            if connected {
                // 回删已写的 restart-ui:失败不留半套标记(重试从零开始)
                let _ = std::fs::remove_file(&restart_marker);
            }
            return Err(self.fail(&format!("写更新标记失败: {e}")));
        }
        self.settle_installing(m);
        self.wait_clients_gone().await;
        self.spawn_installer(setup);
        let _ = self.quit.send(true);
        Ok(())
    }

    /// 作业快照冲刷:注册表内每个作业广播一帧 event.transfer(与翻转/进度
    /// 帧同字段面),让已连 UI 在退出前拿到全部作业的终态/现状。
    fn flush_job_frames(&self, state: &DaemonState) {
        for j in state.jobs().snapshot() {
            let _ = self.events.send(EventFrame {
                method: crate::jobs::EV_TRANSFER.into(),
                params: serde_json::json!({
                    "id": j.id,
                    "kind": j.kind,
                    "state": j.state,
                    "files_done": j.files_done,
                    "files_total": j.files_total,
                    "bytes_done": j.bytes_done,
                    "bytes_total": j.bytes_total,
                    "speed_bps": j.speed_bps,
                    "skipped": j.skipped,
                    "error": j.error,
                }),
            });
        }
    }

    /// 等已连客户端归零:250ms 轮询 [`mtdss_ipc::connected_clients`],上界
    /// [`INSTALL_DRAIN`],超时不候(安装器必须走,不能被挂死客户端拖住;
    /// 自身调用方的读半工正分发本 apply,连接死亡要等下一帧事件写失败才
    /// 被冲走——安静期即落到超时路径,属设计内)。
    async fn wait_clients_gone(&self) {
        let deadline = Instant::now() + INSTALL_DRAIN;
        while mtdss_ipc::connected_clients() > 0 && Instant::now() < deadline {
            tokio::time::sleep(INSTALL_DRAIN_POLL).await;
        }
    }

    /// detached spawn 安装器:注入桩优先(测试);生产 = `<setup> /S`,
    /// stdout/stderr 置空 + `DETACHED_PROCESS|CREATE_NO_WINDOW`(安装器
    /// 自带 UI/重启语义,脱离本进程生命周期,不等子进程)。失败仅 warn:
    /// 标记已在盘,下次开机消费按「未完成」自愈,不在此引入更多分支。
    fn spawn_installer(&self, setup: &Path) {
        if let Some(spawn) = self.cfg.spawner.as_ref() {
            spawn(setup.to_path_buf());
            return;
        }
        let mut cmd = std::process::Command::new(setup);
        cmd.arg("/S");
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        use std::os::windows::process::CommandExt as _;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
        if let Err(e) = cmd.spawn() {
            tracing::warn!(%e, path = %setup.display(), "安装器拉起失败(标记保留,下次开机自愈)");
        }
    }

    /// 钉扎拉取(apply 的显式 version 臂):`{base}/win/{v}/manifest.json`,
    /// 验签 + 版本形态校验。URL 路径只接受严格 X.Y.Z(协议面白名单,顺带
    /// 封路径注入)。
    async fn fetch_pinned_manifest(
        &self,
        v: &str,
        vk: &VerifyingKey,
    ) -> Result<UpdateManifest, RpcError> {
        if version_cmp(v, v).is_none() {
            return Err(RpcError::new(
                ERR_INVALID_PARAMS,
                format!("版本非法(须为严格 X.Y.Z): {v}"),
            ));
        }
        let url = format!("{}/win/{v}/manifest.json", self.cfg.base_url.trim_end_matches('/'));
        let client = self.http_client().map_err(|m| self.fail(&m))?;
        let resp = client
            .get(&url)
            .timeout(CHECK_TIMEOUT)
            .send()
            .await
            .map_err(|e| self.fail(&format!("拉取更新清单失败: {e}")))?;
        if !resp.status().is_success() {
            return Err(self.fail(&format!("拉取更新清单失败: HTTP {}", resp.status())));
        }
        let m: UpdateManifest = resp
            .json()
            .await
            .map_err(|e| self.fail(&format!("解析更新清单失败: {e}")))?;
        if !m.verify(vk) {
            return Err(self.fail("清单验签失败"));
        }
        if version_cmp(&m.version, &m.version).is_none() {
            return Err(self.fail("清单版本非法(须为严格 X.Y.Z)"));
        }
        Ok(m)
    }

    /// 流式下载到临时文件,逐块记账 SHA-256 与字节数并更新节流进度。
    /// 返回 Err 时调用方负责删临时件。
    async fn download_asset(&self, asset: &PlatformAsset, tmp: &Path) -> Result<(), String> {
        use tokio::io::AsyncWriteExt as _;
        let client = self.http_client()?;
        let mut resp = client
            .get(&asset.url)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await
            .map_err(|e| format!("下载失败: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("下载失败: HTTP {}", resp.status()));
        }
        if let Some(len) = resp.content_length() {
            if len != asset.size {
                return Err(format!(
                    "下载复核失败: 响应长度 {len} 与清单不符({})",
                    asset.size
                ));
            }
        }
        let mut file = tokio::fs::File::create(tmp)
            .await
            .map_err(|e| format!("创建临时文件失败: {e}"))?;
        let mut hasher = Sha256::new();
        let mut done: u64 = 0;
        loop {
            let chunk = match resp.chunk().await {
                Ok(Some(c)) => c,
                Ok(None) => break,
                Err(e) => return Err(format!("下载中断: {e}")),
            };
            file.write_all(&chunk)
                .await
                .map_err(|e| format!("写入临时文件失败: {e}"))?;
            hasher.update(&chunk);
            done += chunk.len() as u64;
            self.track_progress(done, asset.size);
        }
        file.flush().await.map_err(|e| format!("冲洗临时文件失败: {e}"))?;
        file.sync_all().await.map_err(|e| format!("落盘临时文件失败: {e}"))?;
        drop(file);
        let sha = hex::encode(hasher.finalize());
        if !sha.eq_ignore_ascii_case(&asset.sha256) {
            return Err(format!("下载复核失败: SHA-256 不符(得 {sha})"));
        }
        if done != asset.size {
            return Err(format!(
                "下载复核失败: 字节数不符({done} != {})",
                asset.size
            ));
        }
        Ok(())
    }

    /// HTTP 客户端:自定义 CA(自签 https)+ 连接超时;总超时按请求给
    /// (清单 15s / 下载 600s),构建本身不设全局超时。
    fn http_client(&self) -> Result<reqwest::Client, String> {
        let mut b = reqwest::Client::builder().connect_timeout(CONNECT_TIMEOUT);
        if let Some(ca) = self.cfg.ca.as_deref() {
            let pem = std::fs::read(ca).map_err(|e| format!("读取更新 CA 失败: {e}"))?;
            let cert =
                reqwest::Certificate::from_pem(&pem).map_err(|e| format!("解析更新 CA 失败: {e}"))?;
            b = b.add_root_certificate(cert);
        }
        b.build().map_err(|e| format!("构建 HTTP 客户端失败: {e}"))
    }

    // ---------- 后台循环 ----------

    /// 后台循环:24h 定时(首拍即发 = 启动即查)+ 作业终态事件监听。
    /// 定时臂:未配置静默;auto_check 门内钉扎检查;auto_install 档静默
    /// 直装(仍守空闲门),仅 auto_check 档系统通知。pending_install 在
    /// 两条臂都会重试(空闲门可能已开)。
    pub(crate) async fn run(self: Arc<Self>, state: Arc<DaemonState>) {
        let mut rx = self.events.subscribe();
        let mut tick = tokio::time::interval(CHECK_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = tick.tick() => self.on_tick(&state).await,
                frame = rx.recv() => {
                    // 作业终态(done/failed/cancelled)→ 空闲门可能已开,
                    // pending_install 重试(非 pending 态静默无操作)
                    if let Ok(f) = frame {
                        if f.method == crate::jobs::EV_TRANSFER
                            && matches!(
                                f.params.get("state").and_then(serde_json::Value::as_str),
                                Some("done" | "failed" | "cancelled")
                            )
                        {
                            self.retry_pending_install(&state).await;
                        }
                    }
                }
            }
        }
    }

    /// 定时臂(见 [`UpdateService::run`])。
    async fn on_tick(&self, state: &Arc<DaemonState>) {
        // 未配置(无公钥):不查不装不通知,状态保持
        if self.cfg.pubkey.is_some() && state.update_auto_check() {
            let pre = self.current_state();
            let _ = self.check(false).await; // 失败已落 failed + 事件,循环继续
            let post = self.current_state();
            if post == "available" && pre != "available" {
                if state.update_auto_install() {
                    // 静默直装档:仍守空闲门(门内拒绝自动转 pending_install)
                    let _ = self.apply(None, state).await;
                } else {
                    // 仅自动检查档:系统通知 + 事件(事件已在翻转时广播)
                    let latest = lock_ok(&self.st).latest.clone().unwrap_or_default();
                    crate::notify::notify_one(
                        "MTDSS 更新可用",
                        &format!("新版本 {latest} 已发布,可在更新设置中安装"),
                    );
                }
            }
        }
        self.retry_pending_install(state).await;
    }

    /// pending_install 重试:非 pending 态无操作;apply 内部重走空闲门,
    /// 失败由状态机自然落定(failed/仍 pending),不向外传播。
    async fn retry_pending_install(&self, state: &Arc<DaemonState>) {
        if self.current_state() == "pending_install" {
            let _ = self.apply(None, state).await;
        }
    }
}

// ---------- 开机标记消费(Task 12) ----------

/// 开机标记消费的决策面(纯函数,可测):读标记、比对版本,返回应执行的
/// 动作,**不执行**(不清盘、不通知、不 spawn)。[`consume_boot_markers`]
/// 据此执行——决策与执行分离,两个版本分支与 restart-ui 的判定可单测。
pub(crate) struct BootMarkerActions {
    /// update-in-progress 标记在场(Some(标记版本));None = 不在场。
    pub update_seen: Option<String>,
    /// 标记版本 == 运行中版本(更新成功收尾 → 「已更新到 X」通知)。
    pub updated_ok: bool,
    /// restart-ui 标记在场(清除 + 唤回主窗)。
    pub reopen_ui: bool,
}

/// 读盘并决策(见 [`BootMarkerActions`])。`current` 为运行中 daemon 版本
/// (生产 = UpdateConfig::production().current,测试 = 注入值)。
pub(crate) fn boot_marker_actions(home: &Path, current: &str) -> BootMarkerActions {
    let update_seen = mtdss_ipc::update_in_progress(home);
    let updated_ok = update_seen.as_deref() == Some(current);
    let reopen_ui = mtdss_ipc::restart_ui_marker_path(home).exists();
    BootMarkerActions { update_seen, updated_ok, reopen_ui }
}

/// 开机消费更新标记(仅 Windows 调用;非 Windows 从不写标记):
/// - update-in-progress 在场:等版本 → 清除 + 系统通知「已更新到 X」;
///   不等 → warn「上次更新未完成(目标 X,当前 Y),已清除标记」+ 清除;
/// - restart-ui 在场:清除 + 唤回主窗(上次更新退出前有已连 UI)。
///
/// 调用时序由 `Daemon::start` 保证:IPC 已就绪、托盘/唤起面之前;须先清
/// update 标记再唤窗——唤起守卫读的就是该标记。全程 best-effort:删除
/// 失败仅留痕,绝不拦启动(残留标记下次开机再消费)。
pub(crate) fn consume_boot_markers(home: &Path, current: &str) {
    let actions = boot_marker_actions(home, current);
    if let Some(target) = actions.update_seen {
        if actions.updated_ok {
            crate::notify::notify_one("MTDSS 已更新", &format!("已更新到 {target}"));
            tracing::info!(version = %target, "更新完成,消费 update-in-progress 标记");
        } else {
            tracing::warn!("上次更新未完成(目标 {target},当前 {current}),已清除标记");
        }
        if let Err(e) = std::fs::remove_file(mtdss_ipc::update_marker_path(home)) {
            tracing::warn!(%e, "清除 update-in-progress 标记失败(下次开机重试)");
        }
    }
    if actions.reopen_ui {
        if let Err(e) = std::fs::remove_file(mtdss_ipc::restart_ui_marker_path(home)) {
            tracing::warn!(%e, "清除 restart-ui 标记失败");
        }
        crate::uilaunch::open_main_window();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtdss_ipc::{restart_ui_marker_path, update_in_progress, update_marker_path};

    /// 开机决策面:等版本标记 → update_seen/updated_ok,restart-ui 不在场。
    #[test]
    fn boot_actions_equal_version() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(update_marker_path(home.path()), "1.0.0").unwrap();
        let a = boot_marker_actions(home.path(), "1.0.0");
        assert_eq!(a.update_seen.as_deref(), Some("1.0.0"));
        assert!(a.updated_ok, "等版本必须判更新成功");
        assert!(!a.reopen_ui);
    }

    /// 开机决策面:不等版本标记 → update_seen 在场但 updated_ok=false。
    #[test]
    fn boot_actions_mismatched_version() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(update_marker_path(home.path()), "9.9.9").unwrap();
        let a = boot_marker_actions(home.path(), "1.0.0");
        assert_eq!(a.update_seen.as_deref(), Some("9.9.9"));
        assert!(!a.updated_ok, "不等版本必须判未完成(告警路径)");
        assert!(!a.reopen_ui);
    }

    /// 开机决策面:无标记 → 全空;restart-ui 在场 → reopen_ui(与 update
    /// 标记互不牵连)。
    #[test]
    fn boot_actions_restart_ui_only_and_empty() {
        let home = tempfile::tempdir().unwrap();
        let a = boot_marker_actions(home.path(), "1.0.0");
        assert!(a.update_seen.is_none() && !a.updated_ok && !a.reopen_ui);

        std::fs::write(restart_ui_marker_path(home.path()), "").unwrap();
        let a = boot_marker_actions(home.path(), "1.0.0");
        assert!(a.reopen_ui, "restart-ui 在场必须判唤回主窗");
        assert!(a.update_seen.is_none(), "update 标记不在场不得误报");
        assert_eq!(update_in_progress(home.path()), None);
    }

    /// write_atomic:同目录 `.tmp` 临时件 + rename 就位;落盘内容原样可读,
    /// 且不留 `.tmp` 残留。
    #[test]
    fn write_atomic_roundtrip_and_no_tmp_leftover() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path().join("contract-marker");
        crate::session::write_atomic(&p, "1.2.3").expect("原子写必须成功");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "1.2.3");
        assert!(
            !home.path().join("contract-marker.tmp").exists(),
            "rename 后同目录不得残留 .tmp"
        );
        // 覆盖写:第二次原子写成功且内容替换(重试场景的幂等面)
        crate::session::write_atomic(&p, "2.0.0").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "2.0.0");
    }
}
