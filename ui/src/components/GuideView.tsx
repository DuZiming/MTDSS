/// 未登录引导态:欢迎说明(引导使用下方表单登录)+ 登录表单。
///
/// 流程:填服务器地址 + 邮箱(→ 可选:自签/dev CA 证书文件,daemon 持久化
/// 到待登录槽,verify 复用)→ 「获取验证码」(dev 模式服务器把码回显在
/// 响应里,直接内联展示并预填)→ 填验证码(+可选设备名,默认主机名)
/// → 「登录」→ 成功后由父组件刷新 hello/status/devices。
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { baseName } from "../lib/clipboard";
import { loginRequestCode, loginVerify, pickCaCert, toRpcError } from "../lib/daemon";

export default function GuideView({ onLoggedIn }: { onLoggedIn: () => void }) {
  const { t } = useTranslation();
  const [server, setServer] = useState("");
  const [email, setEmail] = useState("");
  const [caPath, setCaPath] = useState<string | null>(null);
  const [code, setCode] = useState("");
  const [deviceName, setDeviceName] = useState("");
  const [devCodeShown, setDevCodeShown] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

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
      setDevCodeShown(got);
      setCode(got); // 开发期便利:预填验证码
    } catch (e) {
      setError(`${t("guide.errors.requestCode")}:${toRpcError(e).message}`);
    } finally {
      setBusy(false);
    }
  };

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
      await loginVerify(
        email.trim(),
        code.trim(),
        deviceName.trim() ? deviceName.trim() : null,
      );
      onLoggedIn();
    } catch (e) {
      setError(`${t("guide.errors.verify")}:${toRpcError(e).message}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="guide">
      <div className="guide-card">
        <h1 className="guide-title">{t("app.title")}</h1>
        <p className="guide-explain">{t("guide.explain")}</p>

        <div className="login-form">
          <label className="form-row">
            <span className="form-label">{t("guide.form.server")}</span>
            <input
              type="text"
              value={server}
              onChange={(e) => setServer(e.target.value)}
              placeholder={t("guide.form.serverPlaceholder")}
            />
          </label>
          <label className="form-row">
            <span className="form-label">{t("guide.form.email")}</span>
            <input
              type="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              placeholder={t("guide.form.emailPlaceholder")}
            />
          </label>
          <div className="form-row">
            <span className="form-label">{t("guide.form.ca")}</span>
            <div className="ca-field">
              <button type="button" className="btn" onClick={pickCa} disabled={busy}>
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
          <div className="form-row form-actions">
            <span className="form-label" />
            <button
              className="btn"
              onClick={requestCode}
              disabled={busy}
            >
              {busy ? t("guide.form.gettingCode") : t("guide.form.getCode")}
            </button>
          </div>

          {devCodeShown != null && (
            <div className="dev-code">
              {t("guide.form.devCode", { code: devCodeShown })}
            </div>
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
          <label className="form-row">
            <span className="form-label">{t("guide.form.deviceName")}</span>
            <input
              type="text"
              value={deviceName}
              onChange={(e) => setDeviceName(e.target.value)}
              placeholder={t("guide.form.deviceNamePlaceholder")}
            />
          </label>
          <div className="form-row form-actions">
            <span className="form-label" />
            <button className="btn primary" onClick={doLogin} disabled={busy}>
              {busy ? t("guide.form.loggingIn") : t("guide.form.login")}
            </button>
          </div>

          {error != null && <div className="form-error">{error}</div>}
        </div>
      </div>
    </div>
  );
}
