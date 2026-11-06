import { describe, expect, it } from "vitest";

import { modalBusy } from "./modality";

/// 便捷构造:全部模态位全关 = 空闲基线。
function idle() {
  return {
    ctxMenu: null,
    confirmDel: null,
    pendingConflict: null,
    updatePage: null,
    approvalModal: null,
    favName: null,
    favDel: null,
    favPick: null,
    favMemberPick: null,
    favMenu: null,
    favManage: null,
    favDelFor: null,
  };
}

describe("modalBusy(模态占线:任一模态打开即挡全局快捷键/拖入)", () => {
  it("全关 → 不占线(正常操作面)", () => {
    expect(modalBusy(idle())).toBe(false);
  });

  it("更新页关闭态的布尔 false 也不占线(M4b 冒烟回归:App 以 useState(false) 直传)", () => {
    // 曾用 != null 判定:false != null 恒真 → 快捷键/拖入全被静音(smoke §A 实测)
    expect(modalBusy({ ...idle(), updatePage: false })).toBe(false);
  });

  it("每个模态单独打开都占线:右键菜单 / 删除确认 / 冲突对话框 / 更新页 / 审批弹窗", () => {
    // 冲突对话框 = 评审 Important #1 的漏网位:Del 曾在它打开时仍可删选择
    expect(modalBusy({ ...idle(), pendingConflict: { conflicts: [] } })).toBe(true);
    expect(modalBusy({ ...idle(), confirmDel: { device: "d", paths: [] } })).toBe(true);
    expect(modalBusy({ ...idle(), ctxMenu: { x: 0, y: 0 } })).toBe(true);
    // 更新页覆盖层(Task 15):aria-modal 全屏遮罩,同属占线位
    expect(modalBusy({ ...idle(), updatePage: true })).toBe(true);
    // 新终端审批弹窗(V3-1 Task 10):受信终端主窗模态,同属占线位
    expect(modalBusy({ ...idle(), approvalModal: { session_id: "s" } })).toBe(true);
    // 新建/重命名快捷目录对话框(V3-3 Task 8):布尔真值语义同 updatePage
    expect(modalBusy({ ...idle(), favName: true })).toBe(true);
    expect(modalBusy({ ...idle(), favName: false })).toBe(false);
    // 快捷位删除确认梯 / 添加到快捷 picker(V3-3 Task 11):对象|null 同占线
    expect(modalBusy({ ...idle(), favDel: { groups: [] } })).toBe(true);
    expect(modalBusy({ ...idle(), favPick: { device: "d", paths: [] } })).toBe(true);
    // 快捷成员选择层(V3-3 Task 12 拖放/粘贴多成员解析):对象|null 同占线
    expect(modalBusy({ ...idle(), favMemberPick: { favId: "f" } })).toBe(true);
    // 快捷行右键菜单 / 管理屏 / 删除快捷确认(V3-3 Task 13):同占线位
    expect(modalBusy({ ...idle(), favMenu: { id: "f" } })).toBe(true);
    expect(modalBusy({ ...idle(), favManage: { id: "f" } })).toBe(true);
    expect(modalBusy({ ...idle(), favDelFor: "f" })).toBe(true);
  });

  it("多模态叠加 → 仍占线(如冲突对话框上再叠 force 删除确认)", () => {
    expect(
      modalBusy({
        ctxMenu: null,
        confirmDel: { device: "d", paths: ["a"] },
        pendingConflict: { conflicts: [] },
        updatePage: null,
        approvalModal: null,
        favName: null,
        favDel: null,
        favPick: null,
        favMemberPick: null,
        favMenu: null,
        favManage: null,
        favDelFor: null,
      }),
    ).toBe(true);
    expect(
      modalBusy({
        ctxMenu: { x: 1, y: 1 },
        confirmDel: { device: "d", paths: [] },
        pendingConflict: null,
        updatePage: null,
        approvalModal: null,
        favName: null,
        favDel: null,
        favPick: null,
        favMemberPick: null,
        favMenu: null,
        favManage: null,
        favDelFor: null,
      }),
    ).toBe(true);
  });
});
