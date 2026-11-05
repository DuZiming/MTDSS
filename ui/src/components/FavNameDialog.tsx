/// 新建/重命名常用目录对话框(V3-3 Task 8):模态遮罩 + 标题/名称输入/
/// 取消·提交两键,受控组件(选择权完全在调用方,T13 重命名复用)。标题
/// 与提交钮文案按 initial 有无切换:无 initial=新建(「新建常用目录」/
/// 「确定」),有 initial=重命名(「重命名」/「保存」)。Escape 与遮罩
/// 点击均为取消;输入框 Enter 提交;空白名(去首尾空格)与 busy 在途
/// 禁提交,提交值去首尾空格。样式沿 ConfirmDialog 的 .dialog 面,输入行
/// 沿登录表单 .form-row input 形(.fav-name-input)。
import { useEffect, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { useTranslation } from "react-i18next";

import { useFocusTrap } from "../lib/useFocusTrap";

export default function FavNameDialog({
  open,
  initial,
  busy,
  onSubmit,
  onCancel,
}: {
  open: boolean;
  initial?: string;
  busy: boolean;
  onSubmit(name: string): void;
  onCancel(): void;
}) {
  const { t } = useTranslation();
  const ref = useFocusTrap(open);
  const [name, setName] = useState(initial ?? "");
  // 每次打开复位输入:新建态清空、重命名态预填当前名(open 翻转为真才
  // 复位;关闭期间保持,防复位闪烁)。
  useEffect(() => {
    if (open) setName(initial ?? "");
  }, [open, initial]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onCancel]);

  if (!open) return null;
  const rename = (initial ?? "") !== "";
  const title = rename ? t("favorites.renameTitle") : t("favorites.newTitle");
  const submit = () => {
    const v = name.trim();
    if (v === "" || busy) return;
    onSubmit(v);
  };
  const onInputKeyDown = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") submit();
  };
  return (
    <div className="dialog-mask" role="presentation" onClick={onCancel}>
      <div
        ref={ref}
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onClick={(e) => e.stopPropagation()}
      >
        <p className="dialog-title">{title}</p>
        <input
          className="fav-name-input"
          value={name}
          aria-label={t("favorites.nameLabel")}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={onInputKeyDown}
        />
        <div className="dialog-actions">
          <button className="btn" onClick={onCancel}>
            {t("common.cancel")}
          </button>
          <button
            className="btn primary"
            onClick={submit}
            disabled={busy || name.trim() === ""}
          >
            {rename ? t("favorites.submitRename") : t("favorites.submitNew")}
          </button>
        </div>
      </div>
    </div>
  );
}
