/// 预览面板宽度记忆(V4-1 T20):左缘拖柄拖调面板宽(占宿主 .body 百分
/// 比),账号 KV 持久(brief 钉死键 `previewpanel:width`;存值=整数字符
/// 串,viewmode「字面量原串」先例)。clamp 25-70,缺省 40。读写尽力而为
/// (viewmode 同款纪律):kvGet 失败(离线/未登录)→ 缺省,面板不因记忆
/// 失败;kvPut 失败 → 静默(会话内拖调不受损,下次会话落上次成功值)。
import { kvGet, kvPut } from "./daemon";

/// kv 键(brief 钉死)。
export const PANEL_WIDTH_KV_KEY = "previewpanel:width";
/// 缺省宽(%)。mockup 确认点①「默认宽≈38-42%」取中。
export const PANEL_WIDTH_DEFAULT = 40;
/// clamp 下限/上限(%,brief 钉死 25-70)。
export const PANEL_WIDTH_MIN = 25;
export const PANEL_WIDTH_MAX = 70;

/// 宽度夹取:夹进 [25, 70] 并取整;非有限数值(NaN/±Infinity)按缺省
/// 40 兜底,不外抛不放大。
export function clampPanelWidth(pct: number): number {
  if (!Number.isFinite(pct)) return PANEL_WIDTH_DEFAULT;
  return Math.min(PANEL_WIDTH_MAX, Math.max(PANEL_WIDTH_MIN, Math.round(pct)));
}

/// 读取记忆:键不存在(null)或读取失败(离线/未登录)或存值非数一律
/// 缺省(脏值不放大,clamp 收口)。
export async function loadPanelWidth(): Promise<number> {
  try {
    const raw = await kvGet(PANEL_WIDTH_KV_KEY);
    if (raw == null) return PANEL_WIDTH_DEFAULT;
    return clampPanelWidth(Number(raw));
  } catch {
    return PANEL_WIDTH_DEFAULT;
  }
}

/// 写入记忆:值先 clamp 保证 kv 里永远是干净整数字符串;失败静默。
export async function savePanelWidth(pct: number): Promise<void> {
  try {
    await kvPut(PANEL_WIDTH_KV_KEY, String(clampPanelWidth(pct)));
  } catch {
    // 持久化失败静默:会话内拖调仍生效
  }
}

/// 拖调几何(纯函数,单测钉面):左缘拖柄拖到 clientX 时面板应取的宽
/// 百分比 = (宿主右缘 - 光标)/宿主宽 ×100 → clamp。宿主宽 ≤ 0(布局
/// 未就绪/折叠)按缺省兜底。
export function panelWidthAt(clientX: number, hostRight: number, hostWidth: number): number {
  if (hostWidth <= 0) return PANEL_WIDTH_DEFAULT;
  return clampPanelWidth(((hostRight - clientX) / hostWidth) * 100);
}
