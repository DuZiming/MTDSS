/// 详细信息视图列宽(smoke r2b):details 四列表头拖柄拖调列宽的 clamp
/// 与 localStorage 持久化纯逻辑。持久化键 localStorage["mtdss.colw"];
/// 所有 localStorage 访问 try/catch 包裹(隐私模式/存储被禁时静默回退,
/// 不影响任何功能,与 theme.ts 同口径)。
/// 列宽模型(planner r2b 裁定):mtime/type/size 存自定义 px(未拖过的
/// 列走 DEFAULT_COL_WIDTHS 初值基线);名称列恒弹性不存宽——固定布局下
/// 吃剩余宽,拖名称缘由消费侧反向驱动邻列 mtime;其下限由消费侧 CSS
/// min-width 与拖动动态上限双保险,故本模块对 name 只提供下限常量,
/// load/save 均不落地 name 键。

/// details 四列键(与 SortKey 同字面集,独立定义保持本模块自洽)。
export type ColKey = "name" | "mtime" | "type" | "size";

/// 已自定义列宽(px);键缺省 = 该列未拖过、走默认基线。
export type ColWidths = Partial<Record<ColKey, number>>;

/// 持久化键(localStorage)。
export const COLWIDTH_STORAGE_KEY = "mtdss.colw";

/// 各列 clamp 下限(px):名称 140 / 修改日期 110 / 类型 90 / 大小 70。
export const COL_MIN_WIDTHS: Record<ColKey, number> = {
  name: 140,
  mtime: 110,
  type: 90,
  size: 70,
};

/// 统一上限(px):防单列吞没整表。
export const COL_MAX_WIDTH = 999;

/// 未自定义时的初值基线(px):修改日期 170 / 类型 130 / 大小 90;
/// 名称列不预置(恒弹性),故无 name 键。
export const DEFAULT_COL_WIDTHS: Record<Exclude<ColKey, "name">, number> = {
  mtime: 170,
  type: 130,
  size: 90,
};

/// 实际落地持久化的三列(名称列不存宽,见文件头裁定)。
const PERSISTED_KEYS: readonly Exclude<ColKey, "name">[] = [
  "mtime",
  "type",
  "size",
];

/// 列宽夹取:夹进 [该列下限, 999] 并取整(拖动产生的小数 px 归整);
/// 非有限数值(NaN/±Infinity)按该列下限兜底。
export function clampColWidth(key: ColKey, px: number): number {
  const min = COL_MIN_WIDTHS[key];
  if (!Number.isFinite(px)) return min;
  return Math.min(COL_MAX_WIDTH, Math.max(min, Math.round(px)));
}

/// 读自定义列宽:非法 JSON/非平面对象/值非有限数一律丢弃;脏值逐键
/// clamp;未知键与 name 键不落地;存储不可用返回空对象(全列走默认
/// 基线),绝不外抛。
export function loadColWidths(): ColWidths {
  try {
    const raw = localStorage.getItem(COLWIDTH_STORAGE_KEY);
    if (raw == null) return {};
    const parsed: unknown = JSON.parse(raw);
    if (parsed == null || typeof parsed !== "object" || Array.isArray(parsed)) {
      return {};
    }
    const rec = parsed as Record<string, unknown>;
    const out: ColWidths = {};
    for (const key of PERSISTED_KEYS) {
      const v = rec[key];
      if (typeof v === "number" && Number.isFinite(v)) {
        out[key] = clampColWidth(key, v);
      }
    }
    return out;
  } catch {
    // 存储不可用/JSON 损坏 → 全列默认基线(与 theme 兜底同口径)
    return {};
  }
}

/// 写自定义列宽:仅落地 mtime/type/size 三键(name 即使传入也不落),
/// 值先逐键 clamp 保证存储里永远是干净值;写失败(隐私模式/配额)静默
/// ——当次会话内列宽已生效,仅不跨会话记忆。
export function saveColWidths(w: ColWidths): void {
  const out: ColWidths = {};
  for (const key of PERSISTED_KEYS) {
    const v = w[key];
    if (typeof v === "number" && Number.isFinite(v)) {
      out[key] = clampColWidth(key, v);
    }
  }
  try {
    localStorage.setItem(COLWIDTH_STORAGE_KEY, JSON.stringify(out));
  } catch {
    // 写失败静默:会话内仍生效,仅不跨会话记忆
  }
}
