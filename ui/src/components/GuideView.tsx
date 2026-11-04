/// 未登录引导态(V3-1 Task 10 登录面):欢迎说明 + 登录四相位
/// form(邮箱)/ qr(扫码)/ approval(等待审批)/ evict(超限踢选),
/// 转移收口在 lib/login 纯函数 reducer(本组件只发动作、跑轮询副作用)。
///
/// - **form(r2f 简化)**:邮箱 → 「获取验证码」(dev 模式服务器把码回显
///   在响应里,直接内联展示并预填)→ 填验证码 → 「登录」。verify 新形态
///   分臂:ok 直入 / approval 入等待屏 / limit 入踢选屏 / denied·expired
///   回 form 文案态(guide.terminal.*)。
/// - **qr**:login.qr_begin 起会话(qr_url 经 qrcode 包本地渲染 data: PNG,
///   零外部请求,CSP img-src data: 白名单内),2s 轮询 qr_status;过期
///   (轮询 expired 或本地 TTL 兜底)清码出「重新生成」;ok/denied/
///   limit 与 verify 同臂。**扫码 tab 复用 form 的高级选项**(server/CA/
///   设备名同源,无独立设置)。
/// - **approval**:3s 轮询 login.status 收口;或直输受信终端弹窗上的
///   6 位审批码(login.submit_code)一步登录;取消 = login.cancel 回 form。
/// - **evict**:limit 清单逐行「移出」(login.evict,单次凭据);若结果再
///   撞 limit 原位换新清单;「退出登录」取消回 form。
/// 成功后由父组件刷新 hello/status/devices(event.login true 亦会到达)。
import { useEffect, useReducer, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import QRCode from "qrcode";

import { baseName } from "../lib/clipboard";
import {
  loginCancel,
  loginEvict,
  loginQrBegin,
  loginQrStatus,
  loginRequestCode,
  loginStatus,
  loginSubmitCode,
  loginVerify,
  pickCaCert,
  toRpcError,
} from "../lib/daemon";
import {
  loginInitial,
  loginReducer,
  parseLoginStep,
  qrExpiredByTime,
} from "../lib/login";
import ConfirmDialog from "./ConfirmDialog";

/// 生产域名(2026-10-29 起,VPS 47.105.103.246):登录表单 server 预填值,
/// 主面不再露服务器行;dev 自签场景经「高级选项」改地址/补 CA,行为不变。
const DEFAULT_SERVER = "https://www.mtdss.top";

/// 轮询节奏(spec §3.2 扫码 2s 档;审批轮询沿 CLI 同款 3s 档)。
const QR_POLL_MS = 2000;
const APPROVAL_POLL_MS = 3000;
/// 本地到期兜底提前量(秒):服务器 TTL 是权威,轮询通常先到;此余量只在
/// 服务器不可达/无响应时提前收口,避免 UI 挂死在旧码/旧会话上。
const EXPIRY_SLACK_SEC = 5;
/// 服务器响应缺 expires_at 时的审批会话 TTL 兜底(spec §3.4:300s)。
const APPROVAL_TTL_FALLBACK_SEC = 300;

const nowSec = () => Date.now() / 1000;

export default function GuideView({
  onLoggedIn,
  evicted = false,
}: {
  onLoggedIn: () => void;
  /// 本设备被移出账号(event.device_evicted;App 持有,登录成功即清)。
  evicted?: boolean;
}) {
  const { t } = useTranslation();
  const [server, setServer] = useState(DEFAULT_SERVER);
  const [email, setEmail] = useState("");
  const [caPath, setCaPath] = useState<string | null>(null);
  const [code, setCode] = useState("");
  const [deviceName, setDeviceName] = useState("");
  const [devCodeShown, setDevCodeShown] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 高级选项折叠区开关(默认收起):主面只剩邮箱+验证码+登录。
  const [advanced, setAdvanced] = useState(false);
  // 登录四相位(V3-1):转移全在 lib/login reducer,本组件只发动作。
  const [ui, dispatch] = useReducer(loginReducer, undefined, loginInitial);
  // 审批屏的直输审批码(form code 输入框分键:两屏语义不同不复用)。
  const [approvalCode, setApprovalCode] = useState("");
  // 二维码位图(qr_url → data: PNG,异步生成;URL 换新/过期即重算)。
  const [qrDataUrl, setQrDataUrl] = useState<string | null>(null);
  // qr_begin 在途标记(ref 兜防抖:tab 点击与「重新生成」共用入口)。
  const qrBeginRef = useRef(false);
  // 踢选二次确认(final review I3):待移出设备 id(null = 无待确认行)。
  // 移出是不可逆动作(对端立即下线),先弹确认再发 login.evict。
  const [pendingEvict, setPendingEvict] = useState<string | null>(null);

  const pickCa = async () => {
    // 选择器失败静默保持原状:选 CA 是可选增强,不放大为表单错误;
    // 用户取消(path=null)同样不动已选路径。
    try {
      const p = await pickCaCert();
      if (p != null) setCaPath(p);
    } catch {
      // 选择器不可用:保持原状
    }
  };

  const requestCode = async () => {
    setError(null);
    if (!server.trim() || !email.trim()) {
      setError(t("guide.errors.needServerEmail"));
      return;
    }
    setBusy(true);
    try {
      const { code: got } = await loginRequestCode(server.trim(), email.trim(), caPath);
      // 产线邮件直投回 `{}` 无回显(final review C1):`got ?? ""` 兜底,
      // 不得把 undefined 塞进受控输入;空 = 不预填,验证码走手输。
      setDevCodeShown(got ?? null);
      setCode(got ?? ""); // 开发期便利:预填验证码
    } catch (e) {
      setError(`${t("guide.errors.requestCode")}:${toRpcError(e).message}`);
    } finally {
      setBusy(false);
    }
  };

  /// verify 六臂分派(响应族判别在 parseLoginStep;畸形按错误文案呈现)。
  const doLogin = async () => {
    setError(null);
    if (!server.trim() || !email.trim()) {
      setError(t("guide.errors.needServerEmail"));
      return;
    }
    if (!code.trim()) {
      setError(t("guide.errors.needCode"));
      return;
    }
    setBusy(true);
    try {
      const step = parseLoginStep(
        await loginVerify(email.trim(), code.trim(), deviceName.trim() ? deviceName.trim() : null),
      );
      if (step == null) {
        setError(t("guide.errors.verify"));
        return;
      }
      switch (step.status) {
        case "ok":
          onLoggedIn();
          return;
        case "approval":
          dispatch({
            type: "approvalWait",
            sessionId: step.session_id,
            expiresAt: step.expires_at ?? nowSec() + APPROVAL_TTL_FALLBACK_SEC,
          });
          return;
        case "limit":
          dispatch({ type: "evictPick", enrollToken: step.enroll_token, devices: step.devices });
          return;
        case "denied":
          dispatch({ type: "terminal", outcome: "denied" });
          return;
        case "expired":
          dispatch({ type: "terminal", outcome: "expired" });
          return;
        case "pending":
          // verify 不产 pending(daemon 投影面只在轮询形态);守形式报错误。
          setError(t("guide.errors.verify"));
          return;
      }
    } catch (e) {
      setError(`${t("guide.errors.verify")}:${toRpcError(e).message}`);
    } finally {
      setBusy(false);
    }
  };

  /// 扫码会话发起(进 tab 与「重新生成」共用):server 复用高级选项,
  /// CA/设备名同源透传(daemon 记槽,qr_status 凭 token 查)。失败停生成
  /// 中态留错误与重试,不弹回 form(高级选项就在下方,改完原地重来)。
  const beginQr = async () => {
    if (qrBeginRef.current) return;
    setError(null);
    if (!server.trim()) {
      setError(t("guide.errors.needServer"));
      return;
    }
    qrBeginRef.current = true;
    setBusy(true);
    try {
      const b = await loginQrBegin(
        server.trim(),
        caPath,
        deviceName.trim() ? deviceName.trim() : null,
      );
      dispatch({ type: "qrBegin", qrUrl: b.qr_url, token: b.token, expiresAt: b.expires_at });
    } catch (e) {
      setError(`${t("guide.qr.failed")}:${toRpcError(e).message}`);
    } finally {
      qrBeginRef.current = false;
      setBusy(false);
    }
  };

  /// 进扫码 tab:置生成中态再起会话;离开(tab 点击/取消)作废本地续接
  /// 槽(daemon 幂等;服务端会话靠 TTL 收口)。
  const toQrTab = () => {
    if (busy) return;
    setError(null);
    dispatch({ type: "qrEnter" });
    void beginQr();
  };
  const toFormTab = () => {
    if (busy) return;
    setError(null);
    dispatch({ type: "cancel" });
    void loginCancel().catch(() => {});
  };

  /// 中间屏(审批/踢选)取消:回 form + login.cancel 作废槽。
  const cancelFlow = () => {
    dispatch({ type: "cancel" });
    setApprovalCode("");
    setError(null);
    void loginCancel().catch(() => {});
  };

  /// 审批屏直输码收口(受信终端弹窗上的 6 位审批码;approve-code 409 已
  /// 裁决走 daemon 即时领票,响应族与 verify 同臂)。
  const submitApprovalCode = async () => {
    if (ui.phase !== "approval") return;
    const sessionId = ui.wait.sessionId;
    const trimmed = approvalCode.trim();
    setError(null);
    if (!trimmed) {
      setError(t("guide.approval.needCode"));
      return;
    }
    setBusy(true);
    try {
      const step = parseLoginStep(await loginSubmitCode(sessionId, trimmed));
      if (step == null) {
        setError(t("guide.approval.submitFailed"));
        return;
      }
      switch (step.status) {
        case "ok":
          onLoggedIn();
          return;
        case "approval":
        case "pending":
          return; // 受理后仍在收口:3s 轮询面继续
        case "denied":
          dispatch({ type: "terminal", outcome: "denied" });
          return;
        case "expired":
          dispatch({ type: "terminal", outcome: "expired" });
          return;
        case "limit":
          dispatch({ type: "evictPick", enrollToken: step.enroll_token, devices: step.devices });
          return;
      }
    } catch (e) {
      setError(`${t("guide.approval.submitFailed")}:${toRpcError(e).message}`);
    } finally {
      setBusy(false);
    }
  };

  /// 踢选屏移出(单次 enroll_token;结果再撞 limit 时原位换新凭据与清单)。
  /// 行按钮先入二次确认(pendingEvict),此处只收确认后的真动作。
  const evictDevice = async (deviceId: string) => {
    setPendingEvict(null);
    if (ui.phase !== "evict" || busy) return;
    const enrollToken = ui.pick.enrollToken;
    setError(null);
    setBusy(true);
    try {
      const step = parseLoginStep(await loginEvict(enrollToken, deviceId));
      if (step == null) {
        setError(t("guide.evict.failed"));
        return;
      }
      switch (step.status) {
        case "ok":
          onLoggedIn();
          return;
        case "limit":
          dispatch({ type: "evictPick", enrollToken: step.enroll_token, devices: step.devices });
          return;
        case "denied":
          dispatch({ type: "terminal", outcome: "denied" });
          return;
        case "expired":
          dispatch({ type: "terminal", outcome: "expired" });
          return;
        case "approval":
        case "pending":
          return;
      }
    } catch (e) {
      setError(`${t("guide.evict.failed")}:${toRpcError(e).message}`);
    } finally {
      setBusy(false);
    }
  };

  /// 扫码轮询(2s):pending 继续;ok 登录;expired/本地 TTL 兜底 → 生成
  /// 中态出「重新生成」;denied → form 文案态;limit → 踢选屏。瞬时 RPC
  /// 失败静默(下一 tick 自会重试;服务器不可达由本地到期兜底收口)。
  useEffect(() => {
    if (ui.phase !== "qr" || ui.qr == null || ui.expired) return;
    const token = ui.qr.token;
    const expiresAt = ui.qr.expiresAt;
    const id = window.setInterval(() => {
      if (qrExpiredByTime({ qrUrl: "", token, expiresAt }, nowSec() + EXPIRY_SLACK_SEC)) {
        dispatch({ type: "qrExpired" });
        return;
      }
      void (async () => {
        let step;
        try {
          step = parseLoginStep(await loginQrStatus(token));
        } catch {
          return; // 瞬时失败:下一 tick 重试
        }
        if (step == null) return;
        switch (step.status) {
          case "ok":
            onLoggedIn();
            break;
          case "pending":
          case "approval":
            break; // 继续(扫码流程不产 approval,守形式)
          case "denied":
            dispatch({ type: "terminal", outcome: "denied" });
            break;
          case "expired":
            dispatch({ type: "qrExpired" });
            break;
          case "limit":
            dispatch({ type: "evictPick", enrollToken: step.enroll_token, devices: step.devices });
            break;
        }
      })();
    }, QR_POLL_MS);
    return () => window.clearInterval(id);
  }, [
    ui.phase,
    ui.phase === "qr" ? ui.expired : null,
    ui.phase === "qr" ? ui.qr?.token : null,
    ui.phase === "qr" ? ui.qr?.expiresAt : null,
    onLoggedIn,
  ]);

  /// 审批轮询(3s):受信终端弹窗同意后此处领票收口;denied/expired 落
  /// 文案态;limit 转踢选屏;本地到期兜底(cancel 伴发作废槽)。
  useEffect(() => {
    if (ui.phase !== "approval") return;
    const sessionId = ui.wait.sessionId;
    const expiresAt = ui.wait.expiresAt;
    const id = window.setInterval(() => {
      if (expiresAt <= nowSec() + EXPIRY_SLACK_SEC) {
        dispatch({ type: "terminal", outcome: "expired" });
        void loginCancel().catch(() => {});
        return;
      }
      void (async () => {
        let step;
        try {
          step = parseLoginStep(await loginStatus(sessionId));
        } catch {
          return; // 瞬时失败:下一 tick 重试
        }
        if (step == null) return;
        switch (step.status) {
          case "ok":
            onLoggedIn();
            break;
          case "pending":
          case "approval":
            break;
          case "denied":
            dispatch({ type: "terminal", outcome: "denied" });
            break;
          case "expired":
            dispatch({ type: "terminal", outcome: "expired" });
            break;
          case "limit":
            dispatch({ type: "evictPick", enrollToken: step.enroll_token, devices: step.devices });
            break;
        }
      })();
    }, APPROVAL_POLL_MS);
    return () => window.clearInterval(id);
  }, [
    ui.phase,
    ui.phase === "approval" ? ui.wait.sessionId : null,
    ui.phase === "approval" ? ui.wait.expiresAt : null,
    onLoggedIn,
  ]);

  /// qr_url → data: PNG(本地渲染,零外部请求;CSP img-src data: 白名单)。
  /// URL 缺席(生成中/过期清码)即清位图。
  useEffect(() => {
    const url = ui.phase === "qr" && ui.qr != null && ui.qr.qrUrl !== "" ? ui.qr.qrUrl : null;
    if (url == null) {
      setQrDataUrl(null);
      return;
    }
    let stale = false;
    void QRCode.toDataURL(url, { width: 200, margin: 2 })
      .then((d) => {
        if (!stale) setQrDataUrl(d);
      })
      .catch(() => {
        if (!stale) setQrDataUrl(null);
      });
    return () => {
      stale = true;
    };
  }, [ui.phase, ui.phase === "qr" ? ui.qr?.qrUrl : null]);

  const inEntryTabs = ui.phase === "form" || ui.phase === "qr";

  return (
    <div className="guide">
      <div className="guide-card">
        <h1 className="guide-title">{t("app.title")}</h1>
        <p className="guide-explain">{t("guide.explain")}</p>

        {evicted && (
          <div className="guide-evicted" role="status">
            {t("guide.evicted")}
          </div>
        )}

        {/* 入口双 tab(邮箱/扫码):仅在 form/qr 两相位可见;中间屏
            (approval/evict)整卡接管,取消走屏内按钮 */}
        {inEntryTabs && (
          <div className="guide-tabs" role="tablist" aria-label={t("guide.tabsLabel")}>
            <button
              type="button"
              role="tab"
              aria-selected={ui.phase === "form"}
              className={"guide-tab" + (ui.phase === "form" ? " on" : "")}
              onClick={toFormTab}
              disabled={busy}
            >
              {t("guide.tab.form")}
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={ui.phase === "qr"}
              className={"guide-tab" + (ui.phase === "qr" ? " on" : "")}
              onClick={toQrTab}
              disabled={busy}
            >
              {t("guide.tab.qr")}
            </button>
          </div>
        )}

        <div className="login-form">
          {ui.phase === "form" && ui.terminal != null && (
            <div className="dev-code">{t(`guide.terminal.${ui.terminal}`)}</div>
          )}

          {ui.phase === "form" && (
            <>
              <label className="form-row">
                <span className="form-label">{t("guide.form.email")}</span>
                <input
                  type="email"
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                  placeholder={t("guide.form.emailPlaceholder")}
                />
              </label>
              <div className="form-row form-actions">
                <span className="form-label" />
                <button className="btn" onClick={requestCode} disabled={busy}>
                  {busy ? t("guide.form.gettingCode") : t("guide.form.getCode")}
                </button>
              </div>

              {devCodeShown != null && (
                <div className="dev-code">{t("guide.form.devCode", { code: devCodeShown })}</div>
              )}

              <label className="form-row">
                <span className="form-label">{t("guide.form.code")}</span>
                <input
                  type="text"
                  value={code}
                  onChange={(e) => setCode(e.target.value)}
                  placeholder={t("guide.form.codePlaceholder")}
                />
              </label>
              <div className="form-row form-actions">
                <span className="form-label" />
                <button className="btn primary" onClick={doLogin} disabled={busy}>
                  {busy ? t("guide.form.loggingIn") : t("guide.form.login")}
                </button>
              </div>
            </>
          )}

          {ui.phase === "qr" && (
            <div className="qr-screen">
              {ui.qr == null || ui.qr.qrUrl === "" ? (
                // 生成中 / 已过期:占位框;过期示「已过期 + 重新生成」,
                // 失败留错误与重试,均不弹回 form(高级选项就在下方)
                <div className="qr-center">
                  {ui.expired ? (
                    <div className="qr-expired-box">{t("guide.qr.expired")}</div>
                  ) : (
                    <div className="qr-loading">{t("guide.qr.loading")}</div>
                  )}
                  {(ui.expired || error != null) && (
                    <button type="button" className="btn" onClick={() => void beginQr()} disabled={busy}>
                      {t("guide.qr.regenerate")}
                    </button>
                  )}
                </div>
              ) : (
                <div className="qr-box">
                  {qrDataUrl != null && <img src={qrDataUrl} alt={t("guide.qr.hint")} />}
                </div>
              )}
              <p className="qr-hint">{t("guide.qr.hint")}</p>
            </div>
          )}

          {ui.phase === "approval" && (
            <div className="screen-approval">
              <p className="screen-title">{t("guide.approval.title")}</p>
              <p className="screen-hint">{t("guide.approval.hint")}</p>
              <p className="screen-or">{t("guide.approval.orCode")}</p>
              <label className="form-row">
                <span className="form-label">{t("guide.approval.codeLabel")}</span>
                <input
                  type="text"
                  value={approvalCode}
                  onChange={(e) => setApprovalCode(e.target.value)}
                  placeholder={t("guide.approval.codePlaceholder")}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") void submitApprovalCode();
                  }}
                />
              </label>
              <div className="form-row form-actions">
                <span className="form-label" />
                <button className="btn primary" onClick={() => void submitApprovalCode()} disabled={busy}>
                  {busy ? t("guide.approval.submitting") : t("guide.approval.submit")}
                </button>
              </div>
              <p className="screen-waiting">{t("guide.approval.waiting")}</p>
              <div className="form-row form-actions">
                <span className="form-label" />
                <button className="btn" onClick={cancelFlow} disabled={busy}>
                  {t("guide.approval.cancel")}
                </button>
              </div>
            </div>
          )}

          {ui.phase === "evict" && (
            <div className="screen-evict">
              <p className="screen-title">{t("guide.evict.title")}</p>
              <p className="screen-hint">{t("guide.evict.hint")}</p>
              <div className="evict-list">
                {ui.pick.devices.map((d) => (
                  <div key={d.device_id} className={"evict-row" + (d.online ? "" : " offline")}>
                    <span className="evict-name">{d.name ?? d.device_id}</span>
                    <span className="evict-meta">{d.platform}</span>
                    <span className="evict-meta">
                      <span className={d.online ? "dot on" : "dot off"}>●</span>{" "}
                      {d.online ? t("devices.online") : t("devices.offline")}
                    </span>
                    <button
                      type="button"
                      className="btn"
                      disabled={busy}
                      onClick={() => setPendingEvict(d.device_id)}
                    >
                      {t("guide.evict.evict")}
                    </button>
                  </div>
                ))}
                {ui.pick.devices.length === 0 && (
                  <div className="evict-row">
                    <span className="evict-name">{t("devices.empty")}</span>
                  </div>
                )}
              </div>
              <div className="form-row form-actions">
                <span className="form-label" />
                <button className="btn" onClick={cancelFlow} disabled={busy}>
                  {t("guide.evict.cancel")}
                </button>
              </div>
            </div>
          )}

          {/* 高级选项(默认收起;form/qr 两 tab 复用同一份 server/CA/设备
              名):服务器地址(预填 DEFAULT_SERVER 可改)/ CA 证书 / 设备名。
              常规登录零打扰;dev 自签/自建服务器场景展开即达。 */}
          {inEntryTabs && (
            <div className="guide-advanced">
              <button
                type="button"
                className="guide-adv-toggle"
                aria-expanded={advanced}
                onClick={() => setAdvanced((v) => !v)}
              >
                {t("guide.form.advanced")}
              </button>
              {advanced && (
                <div className="guide-adv-body">
                  <label className="form-row">
                    <span className="form-label">{t("guide.form.server")}</span>
                    <input
                      type="text"
                      value={server}
                      onChange={(e) => setServer(e.target.value)}
                      placeholder={t("guide.form.serverPlaceholder")}
                    />
                  </label>
                  <div className="form-row">
                    <span className="form-label">{t("guide.form.ca")}</span>
                    <div className="ca-field">
                      <button type="button" className="btn" onClick={() => void pickCa()} disabled={busy}>
                        {t("guide.form.caPick")}
                      </button>
                      {caPath != null && (
                        <span className="ca-path" title={caPath}>
                          {baseName(caPath)}
                          <button
                            type="button"
                            className="ca-clear"
                            onClick={() => setCaPath(null)}
                            disabled={busy}
                            aria-label={t("guide.form.caClear")}
                          >
                            ×
                          </button>
                        </span>
                      )}
                    </div>
                  </div>
                  <label className="form-row">
                    <span className="form-label">{t("guide.form.deviceName")}</span>
                    <input
                      type="text"
                      value={deviceName}
                      onChange={(e) => setDeviceName(e.target.value)}
                      placeholder={t("guide.form.deviceNamePlaceholder")}
                    />
                  </label>
                </div>
              )}
            </div>
          )}

          {error != null && <div className="form-error">{error}</div>}
        </div>
      </div>

      {/* 踢选二次确认(final review I3):移出不可逆(对端立即下线且需
          重新登录),ConfirmDialog 复用回收站确认梯同一模态件。 */}
      <ConfirmDialog
        open={pendingEvict != null && ui.phase === "evict"}
        title={t("guide.evict.evict")}
        body={t("guide.evict.confirmBody", {
          name:
            ui.phase === "evict"
              ? (ui.pick.devices.find((d) => d.device_id === pendingEvict)?.name ?? pendingEvict)
              : "",
        })}
        confirmLabel={t("guide.evict.evict")}
        onConfirm={() => {
          if (pendingEvict != null) void evictDevice(pendingEvict);
        }}
        onCancel={() => setPendingEvict(null)}
      />
    </div>
  );
}
