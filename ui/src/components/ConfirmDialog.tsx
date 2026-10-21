/// 确认对话框(Task 11 操作面):模态遮罩 + 标题/正文/取消·确认两键。
/// 目前唯一用途是「回收站不可用 → 永久删除」确认梯;Escape 与遮罩点击
/// 均为取消。不持有业务状态,选择权完全在调用方(受控组件)。
import { useEffect } from "react";
import { useTranslation } from "react-i18next";

import { useFocusTrap } from "../lib/useFocusTrap";

export default function ConfirmDialog({
  open,
  title,
  body,
  confirmLabel,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: string;
  body: string;
  confirmLabel: string;
  onConfirm(): void;
  onCancel(): void;
}) {
  const { t } = useTranslation();
  const ref = useFocusTrap(open);
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onCancel]);

  if (!open) return null;
  return (
    <div className="dialog-mask" role="presentation" onClick={onCancel}>
      <div
        ref={ref}
        className="dialog"
        role="alertdialog"
        aria-modal="true"
        aria-label={title}
        onClick={(e) => e.stopPropagation()}
      >
        <p className="dialog-title">{title}</p>
        <p className="dialog-body">{body}</p>
        <div className="dialog-actions">
          <button className="btn" onClick={onCancel}>
            {t("common.cancel")}
          </button>
          <button className="btn primary" onClick={onConfirm}>
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
