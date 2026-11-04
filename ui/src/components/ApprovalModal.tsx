/// 新终端审批弹窗(V3-1 Task 10,受信终端侧主窗模态):event.approval
/// 到达即弹——候选设备名/平台 + 6 位审批码大字 + 同意/拒绝两键。语义与
/// ConfirmDialog 同门:模态遮罩 + 焦点圈;Escape 与遮罩点击均为**关闭不
/// 应答**(不替用户表态,悬置会话由服务端 TTL 收口);不持有业务状态,
/// 应答权完全在调用方(受控组件,App 持 req/busy 并发 approval.respond)。
/// 审批只在前台生效(无推送,伞形裁定):窗口不在前台时事件照达,模态
/// 随下次渲染出现。
import { useEffect } from "react";
import { useTranslation } from "react-i18next";

import type { ApprovalEvent } from "../lib/daemon";
import { useFocusTrap } from "../lib/useFocusTrap";

export default function ApprovalModal({
  req,
  busy,
  onRespond,
  onDismiss,
}: {
  /// 当前待应答的审批请求(null = 不渲染)。
  req: ApprovalEvent | null;
  /// 应答 RPC 在途(两键置灰防连点)。
  busy: boolean;
  onRespond(allow: boolean): void;
  onDismiss(): void;
}) {
  const { t } = useTranslation();
  const ref = useFocusTrap(req != null);
  useEffect(() => {
    if (req == null) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onDismiss();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [req, onDismiss]);

  if (req == null) return null;
  return (
    <div className="dialog-mask" role="presentation" onClick={onDismiss}>
      <div
        ref={ref}
        className="dialog approval-dialog"
        role="alertdialog"
        aria-modal="true"
        aria-label={t("approve.title")}
        onClick={(e) => e.stopPropagation()}
      >
        <p className="dialog-title">{t("approve.title")}</p>
        <p className="dialog-body">{t("approve.body", { name: req.candidate_name })}</p>
        <p className="approval-platform">{t("approve.platform", { platform: req.candidate_platform })}</p>
        <p className="approval-code-label">{t("approve.codeLabel")}</p>
        <p className="approval-code">{req.code}</p>
        <div className="dialog-actions">
          <button className="btn" onClick={() => onRespond(false)} disabled={busy}>
            {t("approve.deny")}
          </button>
          <button className="btn primary" onClick={() => onRespond(true)} disabled={busy}>
            {t("approve.allow")}
          </button>
        </div>
      </div>
    </div>
  );
}
