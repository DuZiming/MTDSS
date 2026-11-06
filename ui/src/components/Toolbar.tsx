/// 工具栏(P4b Task 5 重排,spec §6):新建(激活下拉,返工 §13.2)| 分隔 |
/// 剪切/复制/粘贴/删除(纯图标 30×28,tooltip/aria-label=现 i18n 词,
/// 启用门控不变)| 分隔 | 排序/查看(图标+文字按钮,下拉沿用 MenuGroup
/// 机制)| 弹簧 | 主题循环(IcTheme,T1 props/行为不变)。
///
/// 「新建」下拉(返工 §13.2):V2 占位钮激活为动作菜单(menuitem 语义,
/// 非 radio)——首项「快捷目录」接 App 既有命名对话框创建流;未登录或
/// fav 只读(前向守卫 v>1)整钮禁用,title 说明。菜单体经 newMenuSections
/// 纯函数构建(测试直驱,viewMenuSections 同款纪律),后续新建类入口在
/// 此段追加。
///
/// 排序菜单两组 radio(spec §6/§8):「排序方式」名称/修改日期/类型/大小
/// + 「方向」递增/递减——与表头点击(T6)共享 App 的同一 SortState 状态
/// 源;Escape/外点收起沿用。纯 React 实现:按钮 + 绝对定位菜单层,
/// menuitemradio + aria-checked 标注勾选态。
///
/// 「查看」菜单组织段(V3-3 Task 10):快捷位(fav)追加「组织」段
/// 混合|分组(menuitemradio,checked 随全局偏好 viewOrg);目录/设备位
/// (orgToggleVisible=false)不追加——普通视图零回归。偏好经 localStorage
/// 键 "favViewOrg" 跨会话记忆(load/save 小函数在本模块导出,theme.ts
/// 同款 try/catch 纪律:坏值/存储不可用静默回缺省「混合」)。
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

/// 快捷视图组织方式(V3-3 Task 10):混合 = 多成员合并单列表;分组 =
/// 按成员分节(节头色点+设备名+路径末段)。fav 位独占的全局偏好。
export type ViewOrg = "mixed" | "grouped";

/// 组织方式持久化键(全局偏好,不随快捷条目走)。
const FAV_VIEW_ORG_KEY = "favViewOrg";

/// 读取持久化组织方式:缺失/非法值 → 缺省「混合」;存储不可用(隐私
/// 模式访问即抛)同样回缺省,绝不外抛(theme.ts 同款纪律)。
export function loadFavViewOrg(): ViewOrg {
  try {
    const raw = localStorage.getItem(FAV_VIEW_ORG_KEY);
    if (raw === "mixed" || raw === "grouped") return raw;
  } catch {
    // 存储不可用 → 缺省混合
  }
  return "mixed";
}

/// 持久化组织方式:写失败(隐私模式/配额)静默——会话内已生效,仅不跨
/// 会话记忆(theme.ts 同款,不值得为此打扰用户)。
export function saveFavViewOrg(org: ViewOrg): void {
  try {
    localStorage.setItem(FAV_VIEW_ORG_KEY, org);
  } catch {
    // 写失败静默
  }
}

/// 单个菜单项描述(label + 是否勾选 + 点击回调),由调用方给定序。
/// checked 仅 radio 语义(itemRole=menuitemradio)消费;动作菜单
/// (menuitem,如「新建」)无勾选态,可省。
export interface MenuItem {
  key: string;
  label: string;
  checked?: boolean;
  onPick(): void;
}

/// 下拉菜单分组:组标头(单组菜单可省,如「查看」)+ 该组 radio 项。
export interface MenuSection {
  label?: string;
  items: MenuItem[];
}

/// 「查看」菜单分段(V3-3 Task 10):第一段三视图 radio(既有,不动);
/// 快捷位(orgToggleVisible)追加第二段「组织」混合|分组。纯函数导出供
/// 测试直驱(菜单展开态是 Toolbar 内部 state,SSR 冒烟够不到菜单体);
/// wrap = Toolbar 的 pick 收尾包装(选中即收起菜单,测试传恒等)。
export function viewMenuSections(
  t: (key: string) => string,
  viewMode: ViewMode,
  viewOrg: ViewOrg,
  orgToggleVisible: boolean,
  onViewMode: (mode: ViewMode) => void,
  onViewOrg: (org: ViewOrg) => void,
  wrap: (fn: () => void) => () => void,
): MenuSection[] {
  const sections: MenuSection[] = [
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
        onPick: wrap(() => onViewMode(m)),
      })),
    },
  ];
  if (orgToggleVisible) {
    sections.push({
      label: t("toolbar.viewOrgGroup"),
      items: (["mixed", "grouped"] as const).map((org) => ({
        key: org,
        label: t(org === "mixed" ? "toolbar.orgMixed" : "toolbar.orgGrouped"),
        checked: viewOrg === org,
        onPick: wrap(() => onViewOrg(org)),
      })),
    });
  }
  return sections;
}

/// 「新建」菜单分段(返工 §13.2):动作菜单,首项「快捷目录」→ App 开
/// 既有命名对话框创建流(onNewFav);后续新建类入口在此段追加。纯函数
/// 导出供测试直驱(展开态是 Toolbar 内部 state,SSR 冒烟够不到菜单体)。
export function newMenuSections(
  t: (key: string) => string,
  onNewFav: () => void,
  wrap: (fn: () => void) => () => void,
): MenuSection[] {
  return [
    {
      items: [
        {
          key: "fav",
          label: t("toolbar.newFavDir"),
          onPick: wrap(onNewFav),
        },
      ],
    },
  ];
}

/// 下拉菜单:触发按钮(图标+文字+chevron)+ 展开层(分组项)。
/// open 由 Toolbar 统一持有(同开一个),便于点击外部/Escape 一次收起。
/// itemRole 定菜单项语义:「menuitemradio」=互斥勾选(排序/查看,缺省,
/// 带勾选列+aria-checked);「menuitem」=动作项(返工 §13.2「新建」,
/// 无勾选列)。缺省 radio 保证既有菜单零改动。导出供测试直驱展开态渲染。
export function MenuGroup({
  label,
  icon,
  sections,
  open,
  onToggle,
  itemRole = "menuitemradio",
}: {
  label: string;
  icon: ReactNode;
  sections: MenuSection[];
  open: boolean;
  onToggle(): void;
  /// 菜单项语义:radio(勾选,缺省)| item(动作)。缺省保持排序/查看
  /// 既有标记不变。
  itemRole?: "menuitemradio" | "menuitem";
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
              {sec.items.map((it) =>
                itemRole === "menuitem" ? (
                  <button
                    key={it.key}
                    type="button"
                    role="menuitem"
                    className="tb-menu-item"
                    onClick={() => {
                      it.onPick();
                    }}
                  >
                    <span>{it.label}</span>
                  </button>
                ) : (
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
                ),
              )}
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
  orgToggleVisible,
  viewOrg,
  onViewOrg,
  opsEnabled,
  pasteEnabled,
  newEnabled,
  onNewFav,
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
  /// 混合|分组组织段可见性:仅快捷位(fav)为 true;目录/设备位不追加
  /// 该段(普通视图零回归,V3-3 Task 10)。
  orgToggleVisible: boolean;
  /// 当前组织方式(App 持有,localStorage 全局偏好)。
  viewOrg: ViewOrg;
  /// 组织方式切换回调(App setState + 持久化一体)。
  onViewOrg(org: ViewOrg): void;
  /// 操作按钮(剪切/复制/删除)可用态:非空选择且当前位为目录。
  opsEnabled: boolean;
  /// 粘贴可用态:目录位(非根卷列表)且设备在线(与右键菜单同门控)。
  pasteEnabled: boolean;
  /// 「新建」下拉可用态(返工 §13.2):已登录且 fav 可写(App 算
  /// loggedIn && !favStore.state.readOnly);false 整钮禁用。
  newEnabled: boolean;
  /// 「新建 > 快捷目录」回调:App 开既有命名对话框创建流。
  onNewFav(): void;
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
  const [openMenu, setOpenMenu] = useState<"sort" | "view" | "new" | null>(null);
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
  const viewSections: MenuSection[] = viewMenuSections(
    t,
    viewMode,
    viewOrg,
    orgToggleVisible,
    onViewMode,
    onViewOrg,
    pick,
  );
  /// 「新建」菜单分段(返工 §13.2):动作项,选中即收起(pick 同款收尾)。
  const newSections: MenuSection[] = newMenuSections(t, onNewFav, pick);

  return (
    <div className="toolbar" role="toolbar" aria-label={t("toolbar.label")} ref={rootRef}>
      {/* 新建(返工 §13.2):激活下拉(menuitem 动作语义,首项「快捷目
          录」);未登录/fav 只读整钮禁用,title 说明(原 favoritesNewBlocked
          语义改写)。禁用态退回纯文字钮(无下拉触发标记)。 */}
      {newEnabled ? (
        <MenuGroup
          label={t("toolbar.new")}
          icon={null}
          itemRole="menuitem"
          sections={newSections}
          open={openMenu === "new"}
          onToggle={() => setOpenMenu((m) => (m === "new" ? null : "new"))}
        />
      ) : (
        <button
          type="button"
          className="tb-btn"
          disabled
          title={t("toolbar.newBlocked")}
          aria-label={t("toolbar.new")}
        >
          {t("toolbar.new")}
        </button>
      )}
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
