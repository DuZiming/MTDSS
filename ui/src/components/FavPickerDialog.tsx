/// 「添加到快捷目录」picker(V3-3 Task 11):目录位右键/多选把候选目录
/// 挂进某个快捷。模态遮罩 + 标题/清单(名 + N 个成员副文)/取消·新建
/// 两键,受控组件(选择权完全在调用方):点选条目 → onPick(id)(App 落
/// addMembers);「新建快捷目录」→ onNew(App 开 FavNameDialog,创建成功
/// 后直接把候选加入新快捷并双层关闭——picker 全程保持挂载)。Escape 与
/// 遮罩点击均为关闭。busy = addMembers/新建在途(条目与新建钮禁点)。
/// 空清单渲染空态文案(新建钮是唯一出路)。样式沿 FavNameDialog 的
/// .dialog 卡片面,清单为卡片内滚动列表(.fav-picker-list)。
import { useEffect } from "react";
import { useTranslation } from "react-i18next";

import type { Favorite } from "../lib/favorites";
import { useFocusTrap } from "../lib/useFocusTrap";

export default function FavPickerDialog({
  open,
  favorites,
  busy,
  onPick,
  onNew,
  onClose,
}: {
  open: boolean;
  /// 全部快捷条目(渲染序 = 创建序);空 = 空态文案。
  favorites: Favorite[];
  /// 添加在途(条目与新建钮禁点,防连点双发)。
  busy: boolean;
  onPick(id: string): void;
  onNew(): void;
  onClose(): void;
}) {
  const { t } = useTranslation();
  const ref = useFocusTrap(open);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;
  return (
    <div className="dialog-mask" role="presentation" onClick={onClose}>
      <div
        ref={ref}
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-label={t("favpicker.title")}
        onClick={(e) => e.stopPropagation()}
      >
        <p className="dialog-title">{t("favpicker.title")}</p>
        {favorites.length === 0 ? (
          <p className="dialog-body fav-picker-empty">{t("favpicker.empty")}</p>
        ) : (
          <ul className="fav-picker-list">
            {favorites.map((f) => (
              <li key={f.id}>
                <button
                  className="fav-picker-item"
                  disabled={busy}
                  onClick={() => {
                    if (busy) return;
                    onPick(f.id);
                  }}
                >
                  <span className="fav-picker-name">{f.name}</span>
                  <span className="fav-picker-meta">
                    {t("favpicker.members", { count: f.members.length })}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
        <div className="dialog-actions">
          <button className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn primary" onClick={onNew} disabled={busy}>
            {t("favpicker.new")}
          </button>
        </div>
      </div>
    </div>
  );
}
