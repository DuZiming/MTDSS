import { describe, expect, it } from "vitest";

import { modalBusy } from "./modality";

/// 便捷构造:三个模态位全关 = 空闲基线。
function idle() {
  return { ctxMenu: null, confirmDel: null, pendingConflict: null };
}

describe("modalBusy(模态占线:任一模态打开即挡全局快捷键/拖入)", () => {
  it("三态全关 → 不占线(正常操作面)", () => {
    expect(modalBusy(idle())).toBe(false);
  });

  it("每个模态单独打开都占线:右键菜单 / 删除确认 / 冲突对话框", () => {
    // 冲突对话框 = 评审 Important #1 的漏网位:Del 曾在它打开时仍可删选择
    expect(modalBusy({ ...idle(), pendingConflict: { conflicts: [] } })).toBe(true);
    expect(modalBusy({ ...idle(), confirmDel: { device: "d", paths: [] } })).toBe(true);
    expect(modalBusy({ ...idle(), ctxMenu: { x: 0, y: 0 } })).toBe(true);
  });

  it("多模态叠加 → 仍占线(如冲突对话框上再叠 force 删除确认)", () => {
    expect(
      modalBusy({
        ctxMenu: null,
        confirmDel: { device: "d", paths: ["a"] },
        pendingConflict: { conflicts: [] },
      }),
    ).toBe(true);
    expect(modalBusy({ ctxMenu: { x: 1, y: 1 }, confirmDel: { device: "d", paths: [] }, pendingConflict: null })).toBe(true);
  });
});
