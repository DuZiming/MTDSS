/// 更新页覆盖层(Task 15 / D13 确认更新面):当前/最新版本、notes、下载
/// 进度(复用传输卡片 .tbar 视觉规格:已知进度定宽填充,未知不确定态
/// 动画条)、安装按钮(→ updateApply,空闲门/清单校验在 daemon 侧收口;
/// 安装指定版本输入仅回滚/钉版本场景展开)、失败重试、检查更新(idle 态
/// 手动检查,spec D13「无已知新版时含手动检查」)。
///
/// 模态面:fixed 遮罩 + 居中面板(与 ConfirmDialog 同 mask 形态,z 最高
/// 层),**独立于登录态**(覆盖引导态/目录视图皆可)、不进 nav 栈——
/// 由 App 持 updatePageOpen 状态挂载,托盘拉起(单实例路由/冷启动标记)
/// 与 toast「查看」动作开页;Escape/遮罩点击/×关闭。installing 态显示
/// 「安装中,窗口将自动关闭」(daemon 安装序列收尾 → event.quit → 桥
/// 关窗,页面随 app 退出自然消亡,无需自旋)。
///
/// 状态驱动:快照(App 持有,event.update 经 reducer 收敛 + 挂载时拉一次
/// update.state 对账)是唯一事实源;RPC 错误不单独呈现——daemon 业务失败
/// 落 failed 态(自带 error 文案)经事件/对账到达,桥接层失败(daemon
/// 未连接)由 App 对账兜底,页面只渲染快照。
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { canInstall, formatProgress, updateTierLabel, type UpdateSnapshot } from "../lib/update";

export default function UpdatePage({
  snapshot,
  busy,
  onApply,
  onCheck,
  onClose,
}: {
  /// 更新快照(event.update reducer 收敛;null = 尚未拉到状态)。
  snapshot: UpdateSnapshot | null;
  /// check/apply RPC 在途(按钮置灰防连点)。
  busy: boolean;
  onApply(version?: string): void;
  onCheck(): void;
  onClose(): void;
}) {
  const { t } = useTranslation();
  // 「安装指定版本…」展开态(回滚/钉版本场景;默认收起)。
  const [advanced, setAdvanced] = useState(false);
  const [version, setVersion] = useState("");

  // Escape 关闭(与 ConfirmDialog 同款;输入框聚焦时同样放行——更新页
  // 无不可丢的表单态)。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  const st = snapshot;
  const downloading = st?.state === "downloading";
  const installable = st != null && canInstall(st);

  // 失败重试面:清单缓存在场(daemon fail() 保留)→ 直接重试安装;
  // 否则(检查阶段即失败)重试检查。
  const retry = () => {
    if (st?.latest != null) onApply();
    else onCheck();
  };

  return (
    <div className="dialog-mask" role="presentation" onClick={onClose}>
      <div
        className="dialog update-page"
        role="dialog"
        aria-modal="true"
        aria-label={t("update.title")}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="update-head">
          <span className="update-title">{t("update.title")}</span>
          <button
            type="button"
            className="tcard-close"
            aria-label={t("update.close")}
            onClick={onClose}
          >
            ×
          </button>
        </div>
        {st == null ? (
          <p className="update-tier">{t("update.loading")}</p>
        ) : (
          <>
            <p className="update-tier">{t(updateTierLabel(st))}</p>
            <div className="update-versions">
              <span>
                {t("update.currentVersion")}:{st.current || "…"}
              </span>
              {st.latest != null && (
                <span>
                  {t("update.latestVersion")}:{st.latest}
                </span>
              )}
            </div>
            {downloading && (
              <div className="update-progress">
                {/* .tbar 视觉规格复用(TransferCards 的进度条形;组件内部
                    与传输卡耦合,此处只借 CSS 形):已知进度定宽填充,翻转
                    帧(进度未至)不确定态动画条 */}
                <div
                  className={"tbar" + (st.progress == null ? " indet" : "")}
                  role="progressbar"
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-valuenow={st.progress == null ? undefined : Math.round(st.progress * 100)}
                >
                  {st.progress != null && (
                    <div className="tbar-fill" style={{ width: `${st.progress * 100}%` }} />
                  )}
                </div>
                <div className="tcard-meta">
                  <span>{formatProgress(st.progress)}</span>
                </div>
              </div>
            )}
            {st.notes != null && st.notes !== "" && (
              <div className="update-notes">
                <p className="update-notes-title">{t("update.notes")}</p>
                <p className="update-notes-body">{st.notes}</p>
              </div>
            )}
            {st.state === "failed" && st.error != null && (
              <p className="update-error">{st.error}</p>
            )}
            {(st.state === "idle" || installable || st.state === "failed") && (
              <div className="update-actions">
                {st.state === "idle" && (
                  <button type="button" className="btn primary" disabled={busy} onClick={onCheck}>
                    {busy ? t("update.checking") : t("update.check")}
                  </button>
                )}
                {/* 安装门控(canInstall)= available/ready/pending_install;
                    failed 不在内(走重试面),downloading/applying/
                    installing 无按钮(状态行已说明) */}
                {installable && (
                  <button
                    type="button"
                    className="btn primary"
                    disabled={busy}
                    onClick={() => onApply()}
                  >
                    {st.state === "pending_install" ? t("update.installNow") : t("update.install")}
                  </button>
                )}
                {st.state === "failed" && (
                  <button type="button" className="btn primary" disabled={busy} onClick={retry}>
                    {/* busy 只置灰不换文案:重试可能是 apply(清单缓存在)
                        也可能是 check,在途身份由状态翻转呈现 */}
                    {t("update.retry")}
                  </button>
                )}
              </div>
            )}
            {/* 回滚/钉版本面(可选版本输入,默认收起):显式版本走 daemon
                钉扎 URL,降级被接受 */}
            <div className="update-advanced">
              {advanced ? (
                <div className="update-adv-row">
                  <input
                    className="update-adv-input"
                    value={version}
                    placeholder={t("update.otherVersionHint")}
                    aria-label={t("update.otherVersionHint")}
                    onChange={(e) => setVersion(e.target.value)}
                  />
                  <button
                    type="button"
                    className="btn"
                    disabled={busy || version.trim() === ""}
                    onClick={() => onApply(version.trim())}
                  >
                    {t("update.installVersion")}
                  </button>
                </div>
              ) : (
                <button
                  type="button"
                  className="update-adv-toggle"
                  onClick={() => setAdvanced(true)}
                >
                  {t("update.otherVersion")}
                </button>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
