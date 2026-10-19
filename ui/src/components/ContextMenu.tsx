/// 右键菜单(Task 11 操作面,spec §4.1)。条目集按触发面二分:
/// - 空白处:粘贴(置灰占位)/ 刷新;
/// - 条目上:打开(仅目录且单选)/ 剪切 Ctrl+X / 复制 Ctrl+C /
///   粘贴(置灰占位)/ 删除 Del。
/// 剪切/复制为 Task 12 内部剪贴板的占位:呈现可用态但点击为 no-op
/// (title 提示「即将提供」);粘贴置灰占位,Task 12 接线。
/// 定位为触发点视口坐标,按估算菜单尺寸钳制防出屏;点击外部/Escape 关闭。
import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";

import type { FsEntry } from "../lib/browse";

/// 右键菜单打开态:触发点视口坐标 + 命中条目(null = 空白处菜单)。
/// 条目上打开时调用方已保证该条目在选中集中(未选中则先单选)。
export interface CtxMenuState {
  x: number;
  y: number;
  target: FsEntry | null;
}

/// 单个菜单项:label 主体 + 可选右侧快捷键标注 + 可用态 + 点击回调。
interface CtxItem {
  key: string;
  label: string;
  keys?: string;
  disabled?: boolean;
  title?: string;
  onPick?(): void;
}

export default function ContextMenu({
  menu,
  canOpen,
  onClose,
  onOpen,
  onDelete,
  onRefresh,
}: {
  menu: CtxMenuState | null;
  /// 「打开」项是否呈现:目标为目录且当前为单选(多选/文件不呈现)。
  canOpen: boolean;
  onClose(): void;
  onOpen(entry: FsEntry): void;
  onDelete(): void;
  onRefresh(): void;
}) {
  const { t } = useTranslation();
  const rootRef = useRef<HTMLDivElement>(null);

  /// 打开期间:点击菜单外部任意处收起;Escape 收起。收起态不挂监听。
  useEffect(() => {
    if (menu == null) return;
    const onDown = (e: globalThis.MouseEvent) => {
      if (rootRef.current != null && !rootRef.current.contains(e.target as Node)) {
        onClose();
      }
    };
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [menu, onClose]);

  if (menu == null) return null;

  /// 触发点即菜单左上角;按估算尺寸(宽 190/高 240)钳制在视口内。
  const left = Math.max(0, Math.min(menu.x, window.innerWidth - 190));
  const top = Math.max(0, Math.min(menu.y, window.innerHeight - 240));

  const stub = t("ctxmenu.stubHint");
  const pasteItem: CtxItem = {
    key: "paste",
    label: t("ctxmenu.paste"),
    disabled: true,
    title: stub,
  };
  const cutItem: CtxItem = {
    key: "cut",
    label: t("ctxmenu.cut"),
    keys: t("ctxmenu.cutKeys"),
    title: stub,
    onPick: () => {}, // Task 12 接线内部剪贴板
  };
  const copyItem: CtxItem = {
    key: "copy",
    label: t("ctxmenu.copy"),
    keys: t("ctxmenu.copyKeys"),
    title: stub,
    onPick: () => {}, // Task 12 接线内部剪贴板
  };
  const items: CtxItem[] =
    menu.target == null
      ? [pasteItem, { key: "refresh", label: t("ctxmenu.refresh"), onPick: onRefresh }]
      : [
          ...(canOpen
            ? [
                {
                  key: "open",
                  label: t("ctxmenu.open"),
                  onPick: () => onOpen(menu.target as FsEntry),
                },
              ]
            : []),
          cutItem,
          copyItem,
          pasteItem,
          {
            key: "delete",
            label: t("ctxmenu.delete"),
            keys: t("ctxmenu.deleteKeys"),
            onPick: onDelete,
          },
        ];

  return (
    <div
      className="ctx-menu"
      role="menu"
      aria-label={menu.target == null ? t("ctxmenu.menuLabelBlank") : t("ctxmenu.menuLabelEntry")}
      style={{ left, top }}
      ref={rootRef}
    >
      {items.map((it) => (
        <button
          key={it.key}
          role="menuitem"
          className="ctx-item"
          disabled={it.disabled}
          title={it.title}
          onClick={() => {
            if (it.disabled) return;
            it.onPick?.();
            onClose();
          }}
        >
          <span>{it.label}</span>
          {it.keys != null && (
            <span className="ctx-keys" aria-hidden="true">
              {it.keys}
            </span>
          )}
        </button>
      ))}
    </div>
  );
}
