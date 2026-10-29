/// 详细信息视图列宽(r2b 建模块,r2d 改「调谁动谁」模型):details 四列
/// 表头拖柄拖调列宽的 clamp 与 localStorage 持久化纯逻辑。持久化键
/// localStorage["mtdss.colw"];所有 localStorage 访问 try/catch 包裹
/// (隐私模式/存储被禁时静默回退,不影响任何功能,与 theme.ts 同口径)。
/// 列宽模型(r2d 用户裁定):四列各持独立 px——拖哪列只改哪列,其余列
/// 纹丝不动;名称列也持久化自己的宽度(有值才存,未拖过保持弹性吃剩余
/// 空间);表格 min-width = sumColWidths(四列当前宽之和,名称未钉按其
/// clamp 下限计),总和超出可视宽时容器出横向滚动条,不挤压任何列。

/// details 四列键(与 SortKey 同字面集,独立定义保持本模块自洽)。
export type ColKey = "name" | "mtime" | "type" | "size";

/// 已自定义列宽(px);键缺省 = 该列未拖过(mtime/type/size 走默认基线,
/// 名称保持弹性)。
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

/// 未自定义时的初值基线(px):修改日期 170 / 类型 130 / 大小 90;名称列
/// 不预置(未拖过恒弹性填满其余空间),故无 name 键。
export const DEFAULT_COL_WIDTHS: Record<Exclude<ColKey, "name">, number> = {
  mtime: 170,
  type: 130,
  size: 90,
};

/// 持久化/求和遍历序(四列;名称列 r2d 起也落地,有值才存)。
const COL_KEYS: readonly ColKey[] = ["name", "mtime", "type", "size"];

/// 列宽夹取:夹进 [该列下限, 999] 并取整(拖动产生的小数 px 归整);
/// 非有限数值(NaN/±Infinity)按该列下限兜底。
export function clampColWidth(key: ColKey, px: number): number {
  const min = COL_MIN_WIDTHS[key];
  if (!Number.isFinite(px)) return min;
  return Math.min(COL_MAX_WIDTH, Math.max(min, Math.round(px)));
}

/// 该列当前宽的回落值:已钉(有限数)→ clamp 后的值;名称未钉 → clamp
/// 下限(求和口径,裁定);其余列未钉 → 默认基线。
function effectiveWidth(key: ColKey, w: ColWidths): number {
  const v = w[key];
  if (v != null && Number.isFinite(v)) return clampColWidth(key, v);
  return key === "name" ? COL_MIN_WIDTHS.name : DEFAULT_COL_WIDTHS[key];
}

/// 表格 min-width 口径(r2d):四列当前宽之和——名称未钉按其下限 140 计,
/// 其余未钉按默认基线;入参逐键 clamp,脏值不放大。容器宽于总和时表格
/// 仍 100% 拉伸,窄于总和时出横向滚动条。
export function sumColWidths(w: ColWidths): number {
  return COL_KEYS.reduce((s, k) => s + effectiveWidth(k, w), 0);
}

/// 读自定义列宽:非法 JSON/非平面对象/值非有限数一律丢弃;脏值逐键
/// clamp;未知键不落地;存储不可用返回空对象(全列走回落值),绝不外抛。
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
    for (const key of COL_KEYS) {
      const v = rec[key];
      if (typeof v === "number" && Number.isFinite(v)) {
        out[key] = clampColWidth(key, v);
      }
    }
    return out;
  } catch {
    // 存储不可用/JSON 损坏 → 全列回落值(与 theme 兜底同口径)
    return {};
  }
}

/// 写自定义列宽:四键有值才落(缺省键不写=该列未拖过),值先逐键 clamp
/// 保证存储里永远是干净值;写失败(隐私模式/配额)静默——当次会话内
/// 列宽已生效,仅不跨会话记忆。
export function saveColWidths(w: ColWidths): void {
  const out: ColWidths = {};
  for (const key of COL_KEYS) {
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

