/// 预览面板宽度记忆(field 定稿 2026-11-23:**px 制**(弃百分比——宽屏上
/// 百分比过大):默认 300px,clamp [240,560],kv 键沿用 `previewpanel:width`
/// 存整数字符串;旧百分比值(≤100)装入时按脏值换缺省(带内自动迁移)。
/// 读写尽力而为:读失败→缺省;写失败→静默(会话内拖调不受损)。
import { kvGet, kvPut } from "./daemon";

/// kv 键(沿用;值域语义换 px)。
export const PANEL_WIDTH_KV_KEY = "previewpanel:width";
/// 缺省宽(px)。
export const PANEL_WIDTH_DEFAULT = 300;
/// clamp 下限/上限(px)。
export const PANEL_WIDTH_MIN = 240;
export const PANEL_WIDTH_MAX = 560;

/// 宽度夹取:夹进 [240, 560] 取整;非有限数值按缺省兜底。
export function clampPanelWidth(px: number): number {
  if (!Number.isFinite(px)) return PANEL_WIDTH_DEFAULT;
  return Math.min(PANEL_WIDTH_MAX, Math.max(PANEL_WIDTH_MIN, Math.round(px)));
}

/// 读取记忆:缺键/读败/非数/旧百分比值(≤100,百分比时代残留)一律缺省。
export async function loadPanelWidth(): Promise<number> {
  try {
    const raw = await kvGet(PANEL_WIDTH_KV_KEY);
    if (raw == null) return PANEL_WIDTH_DEFAULT;
    const v = Number(raw);
    if (!Number.isFinite(v) || v <= 100) return PANEL_WIDTH_DEFAULT; // 旧%残留=脏值
    return clampPanelWidth(v);
  } catch {
    return PANEL_WIDTH_DEFAULT;
  }
}

/// 写入记忆:先 clamp 落干净整数串;失败静默。
export async function savePanelWidth(px: number): Promise<void> {
  try {
    await kvPut(PANEL_WIDTH_KV_KEY, String(clampPanelWidth(px)));
  } catch {
    // 持久化失败静默:会话内拖调仍生效
  }
}

/// 拖调几何(纯函数,单测钉面):左缘拖柄拖到 clientX 时面板宽(px)=
/// 宿主右缘 − 光标(右缘锚定,向左拖变宽)→ clamp。宿主几何不可得按缺省。
export function panelWidthAt(clientX: number, hostRight: number, _hostWidth: number): number {
  return clampPanelWidth(hostRight - clientX);
}
