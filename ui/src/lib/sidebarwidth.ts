/// 侧栏宽度(返工裁定五 §17.4):右缘拖柄拖调侧栏壳宽的 clamp 与
/// localStorage 持久化纯逻辑,纪律试抽自 colwidths.ts——所有 localStorage
/// 访问 try/catch 包裹(隐私模式/存储被禁时静默回退,不影响任何功能,
/// 与 theme.ts 同口径)。持久化键 localStorage["sidebarWidth"](§17.4
/// 键面钦定);**与主题无关**:键里只有一个 px 数值,无任何主题耦合——
/// 未自定义(load 为 null)时壳宽走逐主题默认 var(--side-w)(A/C 196、
/// B 188,静置视觉与存档逐像素同款),一旦拖过,自定义 px 跨主题恒定。

/// 持久化键(localStorage;§17.4 钦定键面 sidebarWidth)。
export const SIDEBAR_WIDTH_STORAGE_KEY = "sidebarWidth";

/// clamp 下限/上限(px):160 保树行文字可用,420 防侧栏吞没主区。
export const SIDEBAR_MIN_WIDTH = 160;
export const SIDEBAR_MAX_WIDTH = 420;

/// 非有限数值(NaN/±Infinity)clamp 的兜底宽(px):取 A/C 主题默认
/// 196(--side-w 存档值)。正常路径到不了这里——脏值在 load 即整单
/// 丢弃返回 null,兜底仅防 save 侧意外传入。
export const DEFAULT_SIDEBAR_WIDTH = 196;

/// 宽度夹取:夹进 [160, 420] 并取整(拖动产生的小数 px 归整);非有限
/// 数值按默认宽 196 兜底,不外抛不放大。
export function clampSidebarWidth(px: number): number {
  if (!Number.isFinite(px)) return DEFAULT_SIDEBAR_WIDTH;
  return Math.min(SIDEBAR_MAX_WIDTH, Math.max(SIDEBAR_MIN_WIDTH, Math.round(px)));
}

/// 读自定义侧栏宽:无记录/非法 JSON/非数值一律 → null(语义=「未自
/// 定义」,壳宽回落逐主题默认 var(--side-w),与 colwidths 空对象的
/// 回落同构——回落值不在存储里造数);合法数值 clamp 后返回(脏值不
/// 放大);存储不可用同样 null,绝不外抛。
export function loadSidebarWidth(): number | null {
  try {
    const raw = localStorage.getItem(SIDEBAR_WIDTH_STORAGE_KEY);
    if (raw == null) return null;
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed === "number" && Number.isFinite(parsed)) {
      return clampSidebarWidth(parsed);
    }
    return null;
  } catch {
    // 存储不可用/JSON 损坏 → 未自定义(与 theme/colwidths 兜底同口径)
    return null;
  }
}

/// 写自定义侧栏宽:值先 clamp 保证存储里永远是干净值(数值 JSON,如
/// "280");写失败(隐私模式/配额)静默——当次会话内宽度已生效,仅
/// 不跨会话记忆。
export function saveSidebarWidth(px: number): void {
  try {
    localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, JSON.stringify(clampSidebarWidth(px)));
  } catch {
    // 写失败静默:会话内仍生效,仅不跨会话记忆
  }
}
