/// 侧栏快捷行右键菜单(V3-3 Task 13):轻量浮层菜单——props 即全部
/// ({x, y, items, onClose}),不持业务语义(项集由 App 按目标快捷装配:
/// 打开/重命名/管理成员/删除快捷,danger=true 红字「删除快捷」)。定位为
/// 触发点视口坐标,按估算尺寸钳制防出屏(ContextMenu 同款手法);点击
/// 外部/Escape 关闭,无焦点陷阱(ContextMenu 先例一致,不另发明 a11y 面)。
/// SSR(纯字符串渲染)无 window:钳制以 0 兜底,只影响静态标记坐标。
import { useEffect, useRef } from "react";

/// 单个菜单项:label 主体 + danger 红字(删除快捷)+ 点击回调。
export interface TreeNodeMenuItem {
  key: string;
  label: string;
  danger?: boolean;
  onPick(): void;
}

export default function TreeNodeMenu({
  x,
  y,
  items,
  onClose,
}: {
  x: number;
  y: number;
  items: TreeNodeMenuItem[];
  onClose(): void;
}) {
  const rootRef = useRef<HTMLDivElement>(null);

  /// 打开期间:点击菜单外部任意处收起;Escape 收起(ContextMenu 同款)。
  useEffect(() => {
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
  }, [onClose]);

  // 防御:空清单不渲染(目标快捷已被删等;App 侧装配出空清单即不占屏)。
  if (items.length === 0) return null;

  /// 触发点即菜单左上角;按估算尺寸(宽 160/行高 34+padding 8)钳制在视
  /// 口内。SSR 无 window:退化为不钳制(0 兜底),仅影响静态标记。
  const vw = typeof window === "undefined" ? 0 : window.innerWidth;
  const vh = typeof window === "undefined" ? 0 : window.innerHeight;
  const left = Math.max(0, Math.min(x, vw - 160));
  const top = Math.max(0, Math.min(y, vh - items.length * 34 - 8));

  return (
    <div className="tree-menu" role="menu" style={{ left, top }} ref={rootRef}>
      {items.map((it) => (
        <button
          key={it.key}
          role="menuitem"
          className={"tree-menu-item" + (it.danger ? " danger" : "")}
          onClick={() => {
            it.onPick();
            onClose();
          }}
        >
          <span>{it.label}</span>
        </button>
      ))}
    </div>
  );
}
