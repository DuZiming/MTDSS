/// 右键菜单(Task 11 操作面,spec §4.1;Task 12 接线剪贴板与下载)。
/// 条目集按触发面二分:
/// - 空白处:粘贴(canPaste 门控)/ 刷新;
/// - 条目上:打开(仅目录且单选)/ 剪切 Ctrl+X / 复制 Ctrl+C /
///   粘贴 / 删除 Del / 下载到本机…(仅远端设备条目)。
/// Task 12:剪切/复制写入内部剪贴板(MTDSS 不写系统剪贴板——Explorer
/// 粘贴为 V1 反目标);粘贴经 App 的分派入口(内部剪贴板 → 五态分派,
/// 空则转读系统 CF_HDROP 上传);下载到本机 = 目录选择器 → download 作业。
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
  canPaste,
  canDownload,
  onClose,
  onOpen,
  onCut,
  onCopy,
  onPaste,
  onDelete,
  onDownloadToLocal,
  onRefresh,
}: {
  menu: CtxMenuState | null;
  /// 「打开」项是否呈现:目标为目录且当前为单选(多选/文件不呈现)。
  canOpen: boolean;
  /// 「粘贴」项可用态:目录位(非根卷列表)且设备在线。粘贴可用性
  /// 不看内部剪贴板是否为空——空则转读系统剪贴板(可能恰有 CF_HDROP)。
  canPaste: boolean;
  /// 「下载到本机…」项是否呈现:仅远端设备条目(本机文件本就在本机)。
  canDownload: boolean;
  onClose(): void;
  onOpen(entry: FsEntry): void;
  onCut(): void;
  onCopy(): void;
  onPaste(): void;
  onDelete(): void;
  onDownloadToLocal(): void;
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

  /// 触发点即菜单左上角;按估算尺寸(宽 190/高 280)钳制在视口内。
  const left = Math.max(0, Math.min(menu.x, window.innerWidth - 190));
  const top = Math.max(0, Math.min(menu.y, window.innerHeight - 280));

  const pasteTitle = canPaste ? undefined : t("toolbar.pasteNoTargetHint");
  const cutItem: CtxItem = {
    key: "cut",
    label: t("ctxmenu.cut"),
    keys: t("ctxmenu.cutKeys"),
    onPick: onCut,
  };
  const copyItem: CtxItem = {
    key: "copy",
    label: t("ctxmenu.copy"),
    keys: t("ctxmenu.copyKeys"),
    onPick: onCopy,
  };
  const items: CtxItem[] =
    menu.target == null
      ? [
          { key: "paste", label: t("ctxmenu.paste"), disabled: !canPaste, title: pasteTitle, onPick: onPaste },
          { key: "refresh", label: t("ctxmenu.refresh"), onPick: onRefresh },
        ]
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
          { key: "paste", label: t("ctxmenu.paste"), disabled: !canPaste, title: pasteTitle, onPick: onPaste },
          ...(canDownload
            ? [
                {
                  key: "download",
                  label: t("ctxmenu.downloadToLocal"),
                  onPick: onDownloadToLocal,
                },
              ]
            : []),
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
