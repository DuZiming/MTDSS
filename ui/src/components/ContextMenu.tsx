/// 右键菜单(Task 11 操作面,spec §4.1;Task 12 接线剪贴板与下载):
/// - 目录位条目上:打开(仅目录且单选)/ 剪切 Ctrl+X / 复制 Ctrl+C /
///   粘贴 / 删除 Del / 下载到本机…(仅远端设备条目)/ 添加到快捷目录
///   (V3-3 Task 11,目标为目录或全目录多选)/ 快速发送到(V3-6 Task 18,
///   spec §5.2:App 作用域门只在本机目录位下发 quickSend prop → 二级
///   飞出列全部远端设备,离线灰显,点击即直发对端收件箱,无确认);
/// - 空白处:粘贴(canPaste 门控)/ 刷新;
/// - 快捷位条目(V3-3 Task 11,favCtx 标记 + mode="fav"):与目录位同一
///   条目集,差异——剪切/复制仅单源组可用(跨源禁用 + 提示),「下载到
///   本机…」恒呈现(选择非空;本机组在 App 侧自动跳过),「添加到快捷
///   目录」不呈现(已在快捷面),「快速发送到」不呈现(fav 位自带下载/
///   转发语义,spec 限文件/多选行 = 本机文件表)。
/// Task 12:剪切/复制写入内部剪贴板(MTDSS 不写系统剪贴板——Explorer
/// 粘贴为 V1 反目标);粘贴经 App 的分派入口(内部剪贴板 → 五态分派,
/// 空则转读系统 CF_HDROP 上传);下载到本机 = 目录选择器 → download 作业。
/// 定位为触发点视口坐标,按估算菜单尺寸钳制防出屏;点击外部/Escape 关闭。
/// 飞出定位:点击父项时取其视口矩形,右缘越界翻左侧,同样钳制(fixed
/// 子节点视口锚定,主菜单无 transform 不改写 containing block)。
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import type { DeviceInfo } from "../lib/derived";
import type { FsEntry } from "../lib/browse";
import { sortDevices } from "../lib/derived";
import type { MixedRow } from "../lib/mixedview";

/// 右键菜单打开态:触发点视口坐标 + 命中条目(null = 空白处菜单)。
/// 条目上打开时调用方已保证该条目在选中集中(未选中则先单选)。
/// favCtx(V3-3 Task 11):快捷位条目菜单标记,rows = 触发时选中集映射的
/// MixedRow(复合键行,剪切/复制跨源门据此判单源组);缺省 = 目录位。
export interface CtxMenuState {
  x: number;
  y: number;
  target: FsEntry | null;
  favCtx?: { rows: MixedRow[] };
}

/// 单个菜单项:label 主体 + 可选右侧快捷键标注 + 可用态 + 点击回调。
/// submenu(V3-6 Task 18):有二级飞出——点击切换飞出(不关菜单、不触发
/// onPick),渲染右侧箭头与 aria-haspopup/expanded。
export interface CtxItem {
  key: string;
  label: string;
  keys?: string;
  disabled?: boolean;
  title?: string;
  submenu?: boolean;
  onPick?(): void;
}

/// 「快速发送到」目标行(quickSendTargets 产物):设备 id + 显示名 +
/// 可用态(离线 = 禁用,灰显非隐藏)。
export interface QuickSendTarget {
  device_id: string;
  label: string;
  disabled: boolean;
}

/// 快速发送目标装配(spec §5.2,裁量 #11):全部远端设备(selfDeviceId
/// 命中者剔除——本机无发送语义),序沿 sortDevices(本机置顶剔除后即
/// 名称序);离线 disabled(灰显非隐藏)。name 空 → label 兜底 device_id
/// (DeviceTable 同款)。无远端设备 → 空数组(入口随之隐藏)。
export function quickSendTargets(
  devices: DeviceInfo[],
  selfDeviceId: string | null,
): QuickSendTarget[] {
  return sortDevices(devices, selfDeviceId)
    .filter((d) => selfDeviceId == null || d.device_id !== selfDeviceId)
    .map((d) => ({
      device_id: d.device_id,
      label: d.name ?? d.device_id,
      disabled: !d.online,
    }));
}

/// 快速发送菜单项装配(QuickSendFlyout 渲染消费;导出供测试直调钉点击
/// 契约:onPick 即 onQuickSend(deviceId) 本体,无确认/预检中介)。离线项
/// 挂 offlineHint title(既有禁用项 pattern:灰 + title)。
export function quickSendItems(
  targets: QuickSendTarget[],
  offlineHint: string,
  onPick: (deviceId: string) => void,
): CtxItem[] {
  return targets.map((tg) => ({
    key: `qs:${tg.device_id}`,
    label: tg.label,
    disabled: tg.disabled,
    title: tg.disabled ? offlineHint : undefined,
    onPick: () => onPick(tg.device_id),
  }));
}

/// 「快速发送到」二级飞出(spec §5.2):主菜单父项点击时钳制好的视口
/// 坐标定位(fixed;.ctx-menu 基础面 + .ctx-flyout 限高滚动)。设备项
/// 点击 = onQuickSend(deviceId) + 关整菜单(选中即发,无确认)。
/// 导出仅为测试(SSR 冒烟无 DOM 交互,开态经本组件直渲钉面)。
export function QuickSendFlyout({
  targets,
  offlineHint,
  onPick,
  onClose,
  left,
  top,
}: {
  targets: QuickSendTarget[];
  offlineHint: string;
  onPick(deviceId: string): void;
  onClose(): void;
  left: number;
  top: number;
}) {
  const { t } = useTranslation();
  return (
    <div
      className="ctx-menu ctx-flyout"
      role="menu"
      aria-label={t("ctxmenu.quickSend")}
      style={{ left, top }}
    >
      {quickSendItems(targets, offlineHint, onPick).map((it) => (
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
        </button>
      ))}
    </div>
  );
}

export default function ContextMenu({
  menu,
  mode = "dir",
  canOpen,
  canPaste,
  canDownload,
  canAddFav,
  canDelete = true,
  deleteTitle,
  quickSend,
  onClose,
  onOpen,
  onCut,
  onCopy,
  onPaste,
  onDelete,
  onDownloadToLocal,
  onAddFav,
  onRefresh,
}: {
  menu: CtxMenuState | null;
  /// 菜单触发面(V3-3 Task 11):dir = 目录位(缺省,零回归);fav =
  /// 快捷位(剪切/复制跨源禁用、下载恒呈现、无「添加到快捷目录」)。
  mode?: "dir" | "fav";
  /// 「打开」项是否呈现:目标为目录且当前为单选(多选/文件不呈现)。
  canOpen: boolean;
  /// 「粘贴」项可用态:目录位(非根卷列表)且设备在线。粘贴可用性
  /// 不看内部剪贴板是否为空——空则转读系统剪贴板(可能恰有 CF_HDROP)。
  /// 快捷位 T12 前恒传 false(项禁用)。
  canPaste: boolean;
  /// 「下载到本机…」项是否呈现:目录位仅远端设备条目(本机文件本就在
  /// 本机);快捷位选择非空恒呈现(本机组在 App 侧自动跳过)。
  canDownload: boolean;
  /// 「添加到快捷目录」项是否呈现(V3-3 Task 11,仅目录位):目标为
  /// 目录且选择空/含该行,或多选非空且全为目录。
  canAddFav: boolean;
  /// 「删除」项可用态(V3-4 §5-1 远端删除门,App 算;缺省 true = 零
  /// 回归):门关且选择指向远端时 false(项禁用 + deleteTitle 提示)。
  canDelete?: boolean;
  /// 「删除」项禁用时的提示(title;缺省无)。
  deleteTitle?: string;
  /// 「快速发送到」供给(V3-6 Task 18,spec §5.2;缺省 = 入口不呈现,
  /// 零回归):App 作用域门只在本机目录位(dir 位且 device=self)下发。
  /// devices 快照即 App 设备态现值(设备上下线随 listenDevices 重渲,
  /// 菜单/飞出恒反映当前可用态)。多选门同样在 App(入口与选择数无关,
  /// spec「文件/多选行」= 本机文件表行,单选多选同入口)。
  quickSend?: {
    devices: DeviceInfo[];
    selfDeviceId: string | null;
    onPick(deviceId: string): void;
  };
  onClose(): void;
  onOpen(entry: FsEntry): void;
  onCut(): void;
  onCopy(): void;
  onPaste(): void;
  onDelete(): void;
  onDownloadToLocal(): void;
  onAddFav(): void;
  onRefresh(): void;
}) {
  const { t } = useTranslation();
  const rootRef = useRef<HTMLDivElement>(null);
  /// 快速发送飞出开态 + 定位(父项点击时按其视口矩形钳制;菜单重开即
  /// 复位收起)。
  const [qsOpen, setQsOpen] = useState(false);
  const [qsPos, setQsPos] = useState<{ left: number; top: number }>({ left: 0, top: 0 });

  /// 菜单实例更替(每次右键新对象)→ 飞出复位收起。
  useEffect(() => {
    setQsOpen(false);
  }, [menu]);

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

  /// 飞出定位(父项点击时算):飞出宽度估 190/高估 220;父项右缘越界
  /// 翻左侧,上下钳在视口内(与主菜单同「估算尺寸钳制」手法)。
  const flyoutPos = (r: DOMRect): { left: number; top: number } => {
    const W = 190;
    const H = 220;
    const PAD = 6;
    const flip = r.right + W + PAD > window.innerWidth;
    return {
      left: flip ? Math.max(PAD, r.left - W - PAD) : r.right + PAD,
      top: Math.max(PAD, Math.min(r.top - 2, window.innerHeight - H)),
    };
  };

  const pasteTitle = canPaste ? undefined : t("toolbar.pasteNoTargetHint");
  /// 快捷位跨源多选门(V3-3 Task 11):选中集映射行跨 >1 台来源设备 →
  /// 剪切/复制禁用 + 提示(复制/剪切写内部剪贴板按单设备形态,跨源多选
  /// 无法表达;目录位恒 false,零回归)。
  const favRows = menu?.favCtx?.rows;
  const crossSource =
    mode === "fav" && favRows != null && new Set(favRows.map((r) => r.device)).size > 1;
  const crossTitle = crossSource ? t("favorites.crossSourceBlocked") : undefined;
  /// 快速发送目标装配(spec §5.2):prop 在场才装配;空数组(无远端
  /// 设备)→ 入口随之隐藏。
  const qsTargets = quickSend != null ? quickSendTargets(quickSend.devices, quickSend.selfDeviceId) : [];
  const cutItem: CtxItem = {
    key: "cut",
    label: t("ctxmenu.cut"),
    keys: t("ctxmenu.cutKeys"),
    disabled: crossSource,
    title: crossTitle,
    onPick: onCut,
  };
  const copyItem: CtxItem = {
    key: "copy",
    label: t("ctxmenu.copy"),
    keys: t("ctxmenu.copyKeys"),
    disabled: crossSource,
    title: crossTitle,
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
          ...(canAddFav
            ? [
                {
                  key: "addFav",
                  label: t("ctxmenu.addFav"),
                  onPick: onAddFav,
                },
              ]
            : []),
          ...(qsTargets.length > 0
            ? [{ key: "quickSend", label: t("ctxmenu.quickSend"), submenu: true }]
            : []),
          {
            key: "delete",
            label: t("ctxmenu.delete"),
            keys: t("ctxmenu.deleteKeys"),
            disabled: !canDelete,
            title: canDelete ? undefined : deleteTitle,
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
          aria-haspopup={it.submenu ? "menu" : undefined}
          aria-expanded={it.submenu ? qsOpen : undefined}
          onClick={(e) => {
            if (it.disabled) return;
            if (it.submenu) {
              setQsPos(flyoutPos(e.currentTarget.getBoundingClientRect()));
              setQsOpen((v) => !v);
              return; // 切换飞出:不关菜单、不触发 onPick
            }
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
          {it.submenu != null && (
            <span className="ctx-sub" aria-hidden="true">
              ›
            </span>
          )}
        </button>
      ))}
      {qsOpen && quickSend != null && (
        <QuickSendFlyout
          targets={qsTargets}
          offlineHint={t("ctxmenu.quickSendOfflineHint")}
          onPick={quickSend.onPick}
          onClose={onClose}
          left={qsPos.left}
          top={qsPos.top}
        />
      )}
    </div>
  );
}
