/// 面板宽度记忆 px 制钉面(field 定稿 2026-11-23):clamp [240,560]
/// 缺省 300;旧百分比值(≤100)读取时按脏值换缺省;拖调几何=右缘−光标。
import { describe, expect, it, vi } from "vitest";

import {
  PANEL_WIDTH_DEFAULT,
  PANEL_WIDTH_KV_KEY,
  clampPanelWidth,
  loadPanelWidth,
  panelWidthAt,
  savePanelWidth,
} from "./panelwidth";

vi.mock("./daemon", () => ({
  kvGet: (...args: unknown[]) => kvGetMock(...args),
  kvPut: (...args: unknown[]) => kvPutMock(...args),
}));
const kvGetMock = vi.fn();
const kvPutMock = vi.fn();

describe("clampPanelWidth(px 制:240-560,缺省 300)", () => {
  it("界内取整透传;越界夹取;非有限数值 → 缺省", () => {
    expect(clampPanelWidth(380)).toBe(380);
    expect(clampPanelWidth(200)).toBe(240);
    expect(clampPanelWidth(99999)).toBe(560);
    expect(clampPanelWidth(455.6)).toBe(456);
    expect(clampPanelWidth(Number.NaN)).toBe(PANEL_WIDTH_DEFAULT);
  });
});

describe("loadPanelWidth/savePanelWidth(尽力而为;键 previewpanel:width)", () => {
  it("存值整数串返回;kvPut 收干净整串;旧%残留/缺键/读败 → 缺省", async () => {
    kvGetMock.mockResolvedValue("380");
    expect(await loadPanelWidth()).toBe(380);
    expect(kvGetMock).toHaveBeenCalledWith(PANEL_WIDTH_KV_KEY);
    expect(PANEL_WIDTH_KV_KEY).toBe("previewpanel:width");
    await savePanelWidth(99999); // clamp 后 560
    expect(kvPutMock).toHaveBeenCalledWith(PANEL_WIDTH_KV_KEY, "560");
  });
  it("旧百分比值(≤100)与脏值 → 缺省;读抛 → 缺省", async () => {
    kvGetMock.mockResolvedValue("55");
    expect(await loadPanelWidth()).toBe(PANEL_WIDTH_DEFAULT);
    kvGetMock.mockResolvedValue(null);
    expect(await loadPanelWidth()).toBe(PANEL_WIDTH_DEFAULT);
    kvGetMock.mockRejectedValue(new Error("offline"));
    expect(await loadPanelWidth()).toBe(PANEL_WIDTH_DEFAULT);
  });
});

describe("panelWidthAt(px:右缘锚定)", () => {
  it("右缘 − 光标;几何不可得 → 缺省;clamp 收口", () => {
    expect(panelWidthAt(600, 1000, 1000)).toBe(400);
    expect(panelWidthAt(300, 1000, 1000)).toBe(560); // 700 → 上限
    expect(panelAt_nan()).toBe(PANEL_WIDTH_DEFAULT);
  });
});
function panelAt_nan(): number {
  return panelWidthAt(Number.NaN, Number.NaN, 0);
}
