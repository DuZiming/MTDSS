/// 工具栏:设计 §3 的按钮行。「排序」「查看」两个下拉菜单在 M3b 生效
/// (纯 React 实现:按钮 + 绝对定位菜单层,点击外部/Escape 关闭,
/// menuitemradio + aria-checked 标注勾选态);其余按钮保持 disabled 占位。
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

/// 内容区视图三态(与 App 的 viewMode 同构)。
export type ViewMode = "details" | "list" | "tiles";

/// 单个菜单项描述(label + 是否勾选 + 点击回调),由调用方给定序。
interface MenuItem {
  key: string;
  label: string;
  checked: boolean;
  onPick(): void;
}

/// 下拉菜单组:触发按钮 + 展开层。open 由 Toolbar 统一持有(同开一个),
/// 便于点击外部/Escape 一次收起。
function MenuGroup({
  label,
  open,
  items,
  onToggle,
}: {
  label: string;
  open: boolean;
  items: MenuItem[];
  onToggle(): void;
}) {
  return (
    <span className="tb-menu-wrap">
      <button
        className="tb-btn"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={label}
        onClick={onToggle}
      >
        {label} ▾
      </button>
      {open && (
        <div className="tb-menu" role="menu" aria-label={label}>
          {items.map((it) => (
            <button
              key={it.key}
              role="menuitemradio"
              aria-checked={it.checked}
              className="tb-menu-item"
              onClick={() => {
                it.onPick();
              }}
            >
              <span className="tb-check" aria-hidden="true">
                {it.checked ? "✓" : ""}
              </span>
              <span>{it.label}</span>
            </button>
          ))}
        </div>
      )}
    </span>
  );
}

export default function Toolbar({
  sortDir,
  onSortDir,
  viewMode,
  onViewMode,
}: {
  sortDir: "asc" | "desc";
  onSortDir(dir: "asc" | "desc"): void;
  viewMode: ViewMode;
  onViewMode(mode: ViewMode): void;
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

  const sortItems: MenuItem[] = [
    {
      key: "asc",
      label: t("toolbar.sortAsc"),
      checked: sortDir === "asc",
      onPick: pick(() => onSortDir("asc")),
    },
    {
      key: "desc",
      label: t("toolbar.sortDesc"),
      checked: sortDir === "desc",
      onPick: pick(() => onSortDir("desc")),
    },
  ];
  const viewItems: MenuItem[] = [
    {
      key: "tiles",
      label: t("toolbar.viewTiles"),
      checked: viewMode === "tiles",
      onPick: pick(() => onViewMode("tiles")),
    },
    {
      key: "list",
      label: t("toolbar.viewList"),
      checked: viewMode === "list",
      onPick: pick(() => onViewMode("list")),
    },
    {
      key: "details",
      label: t("toolbar.viewDetails"),
      checked: viewMode === "details",
      onPick: pick(() => onViewMode("details")),
    },
  ];

  return (
    <div className="toolbar" role="toolbar" aria-label={t("toolbar.label")} ref={rootRef}>
      <button className="tb-btn" disabled title={t("toolbar.disabledHint")}>
        {t("toolbar.new")} <span className="tb-tag">{t("toolbar.newPlaceholder")}</span>
      </button>
      <span className="tb-sep" />
      <button className="tb-btn" disabled title={t("toolbar.disabledHint")}>
        {t("toolbar.cut")}
      </button>
      <button className="tb-btn" disabled title={t("toolbar.disabledHint")}>
        {t("toolbar.copy")}
      </button>
      <button className="tb-btn" disabled title={t("toolbar.disabledHint")}>
        {t("toolbar.paste")}
      </button>
      <button className="tb-btn" disabled title={t("toolbar.disabledHint")}>
        {t("toolbar.delete")}
      </button>
      <span className="tb-spring" />
      <MenuGroup
        label={t("toolbar.sort")}
        open={openMenu === "sort"}
        items={sortItems}
        onToggle={() => setOpenMenu((m) => (m === "sort" ? null : "sort"))}
      />
      <MenuGroup
        label={t("toolbar.view")}
        open={openMenu === "view"}
        items={viewItems}
        onToggle={() => setOpenMenu((m) => (m === "view" ? null : "view"))}
      />
    </div>
  );
}
