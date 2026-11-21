/// 预览面板宽度记忆(V4-1 T20)单测:clamp 面矩阵 + kv 读写(尽力而为,
/// daemon stub 直驱 viewmode.test 同手法)+ 拖调几何纯函数。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const kvGetMock = vi.fn();
const kvPutMock = vi.fn();

vi.mock("./daemon", () => ({
  kvGet: (...args: unknown[]) => kvGetMock(...args),
  kvPut: (...args: unknown[]) => kvPutMock(...args),
}));

import {
  PANEL_WIDTH_DEFAULT,
  PANEL_WIDTH_KV_KEY,
  PANEL_WIDTH_MAX,
  PANEL_WIDTH_MIN,
  clampPanelWidth,
  loadPanelWidth,
  panelWidthAt,
  savePanelWidth,
} from "./panelwidth";

beforeEach(() => {
  kvGetMock.mockReset();
  kvPutMock.mockReset();
});
afterEach(() => {
  vi.restoreAllMocks();
});

describe("clampPanelWidth(brief 钉死 25-70,缺省 40)", () => {
  it("界内取整透传;越界夹取;非有限数值 → 缺省", () => {
    expect(clampPanelWidth(40)).toBe(40);
    expect(clampPanelWidth(40.4)).toBe(40);
    expect(clampPanelWidth(40.6)).toBe(41);
    expect(clampPanelWidth(10)).toBe(PANEL_WIDTH_MIN);
    expect(clampPanelWidth(90)).toBe(PANEL_WIDTH_MAX);
    expect(clampPanelWidth(PANEL_WIDTH_MIN)).toBe(25);
    expect(clampPanelWidth(PANEL_WIDTH_MAX)).toBe(70);
    expect(clampPanelWidth(Number.NaN)).toBe(PANEL_WIDTH_DEFAULT);
    expect(clampPanelWidth(Number.POSITIVE_INFINITY)).toBe(PANEL_WIDTH_DEFAULT);
  });
});

describe("loadPanelWidth/savePanelWidth(键 previewpanel:width,尽力而为)", () => {
  it("键面锚(brief 钉死);存值整数串 clamp 后返回;kvPut 收干净整串", async () => {
    expect(PANEL_WIDTH_KV_KEY).toBe("previewpanel:width");
    kvGetMock.mockResolvedValue("55");
    await expect(loadPanelWidth()).resolves.toBe(55);
    await savePanelWidth(62.9);
    expect(kvPutMock).toHaveBeenCalledWith("previewpanel:width", "63");
    await savePanelWidth(5);
    expect(kvPutMock).toHaveBeenLastCalledWith("previewpanel:width", "25");
  });

  it("键缺失/存值非数/读取失败 → 缺省 40(面板不因记忆失败)", async () => {
    kvGetMock.mockResolvedValue(null);
    await expect(loadPanelWidth()).resolves.toBe(PANEL_WIDTH_DEFAULT);
    kvGetMock.mockResolvedValue("垃圾");
    await expect(loadPanelWidth()).resolves.toBe(PANEL_WIDTH_DEFAULT);
    kvGetMock.mockRejectedValue(new Error("offline"));
    await expect(loadPanelWidth()).resolves.toBe(PANEL_WIDTH_DEFAULT);
  });

  it("写入失败静默(会话内不受损)", async () => {
    kvPutMock.mockRejectedValue(new Error("offline"));
    await expect(savePanelWidth(50)).resolves.toBeUndefined();
  });
});

describe("panelWidthAt(左缘拖调几何)", () => {
  it("光标越靠左面板越宽;clamp 收口;宿主宽 ≤0 缺省兜底", () => {
    // 宿主宽 1000,右缘 1000:光标 600 → 40%
    expect(panelWidthAt(600, 1000, 1000)).toBe(40);
    expect(panelWidthAt(300, 1000, 1000)).toBe(70); // 70% 上限
    expect(panelWidthAt(990, 1000, 1000)).toBe(25); // 1% → 下限
    expect(panelWidthAt(500, 1000, 0)).toBe(PANEL_WIDTH_DEFAULT);
  });
});
