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

// ---------- 框选(rubber band,Task 11) ----------

/// 视口坐标点(框选锚点/当前点,client 坐标系)。
export interface Point {
  x: number;
  y: number;
}

/// 轴对齐矩形(视口坐标;宽高 > 0)。
export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/// 一行的测量结果:路径 + 该行元素的包围盒(getBoundingClientRect 所得,
/// client 坐标系)。三视图行元素统一携带 data-row-path 供 App 查询测量。
export interface RowRect {
  path: string;
  rect: Rect;
}

/// 两矩形是否相交:开区间重叠判定(仅贴边 = 零面积重叠 = 不相交),
/// 零宽/零高矩形与任何矩形都不相交。
function rectsIntersect(a: Rect, b: Rect): boolean {
  return (
    a.width > 0 &&
    a.height > 0 &&
    b.width > 0 &&
    b.height > 0 &&
    a.x < b.x + b.width &&
    a.x + a.width > b.x &&
    a.y < b.y + b.height &&
    a.y + a.height > b.y
  );
}

/// 框选解析(纯函数):锚点 → 当前点张成的矩形,与各行包围盒做相交判定。
///
/// 语义:
/// - 非零面积框:命中行(与框相交)并入 prev 后返回(求并;App 在框选
///   启动的 mousedown 即清空现选择,故实际调用 prev 恒为空集,行为表现
///   为纯替换、与资源管理器一致。prev 入参保留求并形态,供后续 Ctrl+拖拽
///   累加直接复用)。
/// - 零面积框(按下即抬起,未拖动):显式清空——返回空集。
/// - 锚点无关:函数不读不写 Shift 锚点;「框选启动清锚点」由 App 在
///   mousedown 执行(setAnchorPath(null)),返回值亦不含锚点位。
export function rubberBandSelect(
  anchorXY: Point,
  curXY: Point,
  rowRects: readonly RowRect[],
  prev: ReadonlySet<string>,
): Set<string> {
  const band: Rect = {
    x: Math.min(anchorXY.x, curXY.x),
    y: Math.min(anchorXY.y, curXY.y),
    width: Math.abs(curXY.x - anchorXY.x),
    height: Math.abs(curXY.y - anchorXY.y),
  };
  if (band.width <= 0 || band.height <= 0) return new Set();
  const next = new Set(prev);
  for (const { path, rect } of rowRects) {
    if (rectsIntersect(band, rect)) next.add(path);
  }
  return next;
}
