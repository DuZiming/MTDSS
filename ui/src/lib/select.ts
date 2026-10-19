/// 多选点击解析(纯函数):内容区行点击的三种修饰模式 → 新选择集与新锚点。
/// App 层消费(Task 8),状态栏「已选」计数与 Shift 范围均以当前渲染序
/// (排序 + 过滤后)为准。
///
/// 语义:
/// - 单击:清空为该行单选,锚点移至该行。
/// - Ctrl(+Cmd):该行在选择集中切换(增/减),锚点移至该行。
/// - Shift:当前渲染序中 [锚点, 点击行] 闭区间替换选择;锚点不动(便于
///   连续扩展)。锚点缺失、或已不在当前渲染序中(过滤/重排后失配)→
///   退化为单击语义。
/// Shift 区间替换(非累加)与资源管理器一致;Ctrl+Shift 累加属 V2。

/// 行点击修饰模式:由调用方从事件派生(shiftKey / ctrlKey|metaKey)。
export type ClickMode = "single" | "ctrl" | "shift";

/// 行点击/双击事件的修饰键切片(组件层从 React.MouseEvent 结构化取出,
/// 避免视图层与 React 事件类型耦合)。
export interface RowClickInfo {
  shiftKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
}

/// 事件 → 点击模式:Shift 优先(Ctrl+Shift 并按视为范围扩展)。
export function clickMode(e: RowClickInfo): ClickMode {
  if (e.shiftKey) return "shift";
  if (e.ctrlKey || e.metaKey) return "ctrl";
  return "single";
}

/// 事件是否携带多选修饰键:携带即不视为双击打开(Ctrl+双击不得进入目录)。
export function hasSelectModifier(e: RowClickInfo): boolean {
  return e.shiftKey || e.ctrlKey || e.metaKey;
}

/// 一次点击的解析结果:新选择集与下一锚点路径。
export interface SelectResult {
  selection: Set<string>;
  anchor: string | null;
}

/// 解析一次行点击。orderedPaths 为当前渲染序(排序+过滤后)的路径表;
/// current 为现选择集(只读,不修改入参)。
export function applyClickSelect(
  current: ReadonlySet<string>,
  orderedPaths: readonly string[],
  clickPath: string,
  mode: ClickMode,
  anchorPath: string | null,
): SelectResult {
  if (mode === "ctrl") {
    const next = new Set(current);
    if (next.has(clickPath)) {
      next.delete(clickPath);
    } else {
      next.add(clickPath);
    }
    return { selection: next, anchor: clickPath };
  }
  if (mode === "shift" && anchorPath != null) {
    const a = orderedPaths.indexOf(anchorPath);
    const b = orderedPaths.indexOf(clickPath);
    if (a >= 0 && b >= 0) {
      const lo = Math.min(a, b);
      const hi = Math.max(a, b);
      const next = new Set<string>();
      for (let i = lo; i <= hi; i += 1) next.add(orderedPaths[i]);
      return { selection: next, anchor: anchorPath };
    }
  }
  // 单击,以及 Shift 锚点缺失/失配的退化路径。
  return { selection: new Set([clickPath]), anchor: clickPath };
}
