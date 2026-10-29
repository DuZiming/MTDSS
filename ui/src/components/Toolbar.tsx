/// 工具栏(P4b Task 5 重排,spec §6):新建(V2 占位,disabled)| 分隔 |
/// 剪切/复制/粘贴/删除(纯图标 30×28,tooltip/aria-label=现 i18n 词,
/// 启用门控不变)| 分隔 | 排序/查看(图标+文字按钮,下拉沿用 MenuGroup
/// 机制)| 弹簧 | 主题循环(IcTheme,T1 props/行为不变)。
///
/// 排序菜单两组 radio(spec §6/§8):「排序方式」名称/修改日期/类型/大小
/// + 「方向」递增/递减——与表头点击(T6)共享 App 的同一 SortState 状态
/// 源;Escape/外点收起沿用。纯 React 实现:按钮 + 绝对定位菜单层,
/// menuitemradio + aria-checked 标注勾选态。
///
/// 操作面(Task 11/12):剪切/复制/删除按选中态启用(非空选择且当前位为
/// 目录);删除接删除流程;剪切/复制接内部剪贴板(MTDSS 不写系统剪贴板);
/// 粘贴按 canPaste 启用(目录位且非根卷列表,设备在线;与右键菜单同门控)。
import { useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

import type { SortKey, SortState } from "../lib/browse";
import type { ThemeId } from "../lib/theme";
import {
  IcChevronDown,
  IcCheck,
  IcCopy,
  IcCut,
  IcDelete,
  IcPaste,
  IcSort,
  IcTheme,
  IcView,
} from "./icons";

/// 内容区视图三态(与 App 的 viewMode 同构)。
export type ViewMode = "details" | "list" | "tiles";

/// 单个菜单项描述(label + 是否勾选 + 点击回调),由调用方给定序。
interface MenuItem {
  key: string;
  label: string;
  checked: boolean;
  onPick(): void;
}

/// 下拉菜单分组:组标头(单组菜单可省,如「查看」)+ 该组 radio 项。
interface MenuSection {
  label?: string;
  items: MenuItem[];
}

/// 下拉菜单:触发按钮(图标+文字+chevron)+ 展开层(分组 radio)。
/// open 由 Toolbar 统一持有(同开一个),便于点击外部/Escape 一次收起。
function MenuGroup({
  label,
  icon,
  sections,
  open,
  onToggle,
}: {
  label: string;
  icon: ReactNode;
  sections: MenuSection[];
  open: boolean;
  onToggle(): void;
}) {
  return (
    <span className="tb-menu-wrap">
      <button
        type="button"
        className="tb-btn"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={label}
        onClick={onToggle}
      >
        {icon}
        <span>{label}</span>
        <span className="tb-chev" aria-hidden="true">
          <IcChevronDown />
        </span>
      </button>
      {open && (
        <div className="tb-menu" role="menu" aria-label={label}>
          {sections.map((sec, i) => (
            <div
              key={i}
              className="tb-menu-group"
              role="group"
              aria-label={sec.label}
            >
              {sec.label != null && (
                <div className="tb-menu-group-label" aria-hidden="true">
                  {sec.label}
                </div>
              )}
              {sec.items.map((it) => (
                <button
                  key={it.key}
                  type="button"
                  role="menuitemradio"
                  aria-checked={it.checked}
                  className="tb-menu-item"
                  onClick={() => {
                    it.onPick();
                  }}
                >
                  <span className="tb-check" aria-hidden="true">
                    {it.checked && <IcCheck size={12} />}
                  </span>
                  <span>{it.label}</span>
                </button>
              ))}
            </div>
          ))}
        </div>
      )}
    </span>
  );
}

/// 排序方式四键的展示序(spec §6)。
const SORT_KEYS: SortKey[] = ["name", "mtime", "type", "size"];

export default function Toolbar({
  sortState,
  onSortState,
  viewMode,
  onViewMode,
  opsEnabled,
  pasteEnabled,
  theme,
  onCycleTheme,
  onCut,
  onCopy,
  onPaste,
  onDelete,
}: {
  sortState: SortState;
  onSortState(s: SortState): void;
  viewMode: ViewMode;
  onViewMode(mode: ViewMode): void;
  /// 操作按钮(剪切/复制/删除)可用态:非空选择且当前位为目录。
  opsEnabled: boolean;
  /// 粘贴可用态:目录位(非根卷列表)且设备在线(与右键菜单同门控)。
  pasteEnabled: boolean;
  /// 当前主题 id(供循环按钮的 aria/tooltip 文案;P4b Task 1)。
  theme: ThemeId;
  /// 主题循环回调:App 侧推进 fluent→workbench→dark 回绕。
  onCycleTheme(): void;
  onCut(): void;
  onCopy(): void;
  onPaste(): void;
  onDelete(): void;
}) {
  const { t } = useTranslation();
  /// 当前展开的菜单(至多一个);null = 全部收起。
  const [openMenu, setOpenMenu] = useState<"sort" | "view" | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);

  /// 展开时:点击工具栏外部任意处收起;Escape 收起。收起态不挂监听。
  useEffect(() => {
    if (openMenu == null) return;
    const onDown = (e: globalThis.MouseEvent) => {
      if (rootRef.current != null && !rootRef.current.contains(e.target as Node)) {
        setOpenMenu(null);
      }
    };
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Escape") setOpenMenu(null);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [openMenu]);

  const pick = (fn: () => void) => () => {
    fn();
    setOpenMenu(null);
  };

  /// 排序菜单两组 radio:键组(改 sortState.key)+ 方向组(改 sortState.
  /// dir),各自基于当前 sortState 合成新状态上交(单一状态源在 App)。
  const sortSections: MenuSection[] = [
    {
      label: t("toolbar.sortBy"),
      items: SORT_KEYS.map((k) => ({
        key: k,
        label: t(`toolbar.sortKey.${k}`),
        checked: sortState.key === k,
        onPick: pick(() => onSortState({ ...sortState, key: k })),
      })),
    },
    {
      label: t("toolbar.direction"),
      items: (["asc", "desc"] as const).map((d) => ({
        key: d,
        label: t(d === "asc" ? "toolbar.sortAsc" : "toolbar.sortDesc"),
        checked: sortState.dir === d,
        onPick: pick(() => onSortState({ ...sortState, dir: d })),
      })),
    },
  ];
  const viewSections: MenuSection[] = [
    {
      items: (["tiles", "list", "details"] as const).map((m) => ({
        key: m,
        label: t(
          m === "tiles"
            ? "toolbar.viewTiles"
            : m === "list"
              ? "toolbar.viewList"
              : "toolbar.viewDetails",
        ),
        checked: viewMode === m,
        onPick: pick(() => onViewMode(m)),
      })),
    },
  ];

  return (
    <div className="toolbar" role="toolbar" aria-label={t("toolbar.label")} ref={rootRef}>
      {/* 新建(V2 占位):保留文字+占位 tag,disabled,tooltip=disabledHint */}
      <button type="button" className="tb-btn" disabled title={t("toolbar.disabledHint")}>
        {t("toolbar.new")} <span className="tb-tag">{t("toolbar.newPlaceholder")}</span>
      </button>
      <span className="tb-sep" />
      {/* 剪切/复制/粘贴/删除:纯图标 30×28,aria-label=操作词;门控不变
          (Task 12:剪切/复制写内部剪贴板,MTDSS 不写系统剪贴板;粘贴分派
          入口在 App,空内部剪贴板转读系统 CF_HDROP) */}
      <button
        type="button"
        className="tb-icon-btn"
        disabled={!opsEnabled}
        title={t("toolbar.cut")}
        aria-label={t("toolbar.cut")}
        onClick={onCut}
      >
        <IcCut />
      </button>
      <button
        type="button"
        className="tb-icon-btn"
        disabled={!opsEnabled}
        title={t("toolbar.copy")}
        aria-label={t("toolbar.copy")}
        onClick={onCopy}
      >
        <IcCopy />
      </button>
      <button
        type="button"
        className="tb-icon-btn"
        disabled={!pasteEnabled}
        title={!pasteEnabled ? t("toolbar.pasteNoTargetHint") : t("toolbar.paste")}
        aria-label={t("toolbar.paste")}
        onClick={onPaste}
      >
        <IcPaste />
      </button>
      <button
        type="button"
        className="tb-icon-btn"
        disabled={!opsEnabled}
        title={t("toolbar.delete")}
        aria-label={t("toolbar.delete")}
        onClick={onDelete}
      >
        <IcDelete />
      </button>
      <span className="tb-sep" />
      <MenuGroup
        label={t("toolbar.sort")}
        icon={<IcSort />}
        sections={sortSections}
        open={openMenu === "sort"}
        onToggle={() => setOpenMenu((m) => (m === "sort" ? null : "sort"))}
      />
      <MenuGroup
        label={t("toolbar.view")}
        icon={<IcView />}
        sections={viewSections}
        open={openMenu === "view"}
        onToggle={() => setOpenMenu((m) => (m === "view" ? null : "view"))}
      />
      <span className="tb-spring" />
      {/* 主题循环(P4b Task 1 位置/props 不变,Task 3 换 IcTheme) */}
      <button
        type="button"
        className="tb-icon-btn"
        title={t("theme.cycle", { name: t(`theme.${theme}`) })}
        aria-label={t("theme.cycle", { name: t(`theme.${theme}`) })}
        onClick={onCycleTheme}
      >
        <IcTheme />
      </button>
    </div>
  );
}
