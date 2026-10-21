#![cfg(windows)]
//! daemon 更新核心(M4b Task 11):状态机/检查/下载/验签/空闲门(不 spawn)。
//!
//! 信任模型:清单为 canonical ed25519 签名(core::update::UpdateManifest),
//! 验签公钥是唯一信任锚;验签通过后清单内字段(含资产 URL)即为受信,
//! 不另设 URL 白名单。下载落 `{home}/downloads/mtdss-setup-<ver>.exe`,
//! 临时名 `.part`,SHA-256 与字节数复核通过后才 rename 就位——本任务止于
//! ready,applying/installing 与实际 spawn 属 Task 12。
//!
//! fail-closed 口径(错误分类三分):
//! - **未配置拒绝**(公钥 None):check/apply 一律 Err「更新未配置」,状态
//!   不翻——后台循环同样静默不查不装(否则无钥构建每次启动都翻 failed);
//! - **操作失败**(清单拉取/解析/验签/版本非法/下载/复核/rename):一律
//!   state=failed + `event.update` 广播 + 临时文件即删,系统零改动;
//! - **空闲门拒绝**(有传输作业,含已暂停):code 1 + `data.kind=
//!   JOBS_ACTIVE` + 文案精确,转 pending_install(非 failed),作业终态
//!   事件或下轮定时重试([`UpdateService::run`])。
//!
//! 状态机:`idle → available → downloading → ready`(applying/installing
//! 属 Task 12);分支 failed / pending_install。下载/挂起/就绪期间 check
//! 不改写状态(404/网络错误不得打翻进行中的下载或已就绪的包)。钉扎语义:
//! 定时检查拉 `{base}/win/{current}/manifest.json`,404 = 该版本无更新
//! (常态)→ idle 而非失败;force 检查(RPC update.check)拉
//! `{base}/manifest.json`。apply(显式 version)同走钉扎 URL,M4 无防回滚。
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
use std::path::Path;
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

/// 更新服务配置(base/CA/验签公钥/当前版本)。`pub` 仅为集成测试注入面
/// (`Daemon::start_with_update` 携带;与 flow/ops 的 pub 模块同款口径),
/// 其余构造面(UpdateService、run)保持 pub(crate)。
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
}

impl Default for UpdateConfig {
    /// 失败安全缺省:`.invalid` 占位基址(永不解析)+ 无公钥(未配置,
    /// 一切 check/apply 拒绝)——即使被误用也不可能触达真实端点或放行更新。
    fn default() -> Self {
        Self {
            base_url: "https://mtdss-update.invalid".into(),
            ca: None,
            pubkey: None,
            current: env!("CARGO_PKG_VERSION").into(),
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
        }
    }
}

/// 更新状态机的可变面(锁内持改;全部操作毫秒级,无长阻塞)。std Mutex:
/// 锁内无 await;毒化经 [`lock_ok`] 恢复(与 jobs 同款)。
struct UpdateSt {
    /// idle|available|downloading|ready|pending_install|applying|installing|
    /// failed(后两态属 Task 12,本任务不进入)。
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
}

impl UpdateService {
    pub(crate) fn new(cfg: UpdateConfig, events: tokio::sync::broadcast::Sender<EventFrame>) -> Self {
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
            "downloading" | "pending_install" | "ready"
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

    // ---------- 应用(下载到 ready 为止;spawn 属 Task 12) ----------

    /// 应用更新:idle 门(有传输作业[含已暂停] → JOBS_ACTIVE 拒绝并转
    /// pending_install)→ downloading(节流进度)→ SHA-256/字节数复核 →
    /// ready。version 缺省用已检得的清单缓存;显式 version 走钉扎 URL
    /// (M4 无防回滚,降级被接受)。ready 幂等返回;并发 apply 经锁串行。
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
        if self.current_state() == "ready" {
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
        Ok(self.state())
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
