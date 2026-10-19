import { describe, expect, it } from "vitest";

import {
  baseName,
  dragPlan,
  joinDirName,
  pastePlan,
  type ClipRef,
} from "./clipboard";

/// 便捷构造内部剪贴板引用(brief 钉死最小面 {device, paths, cut})。
function clip(device: string, paths: string[], cut = false): ClipRef {
  return { device, paths, cut };
}

/// 四态分派测试的公共坐标:self=本机、A/B=两台远端设备;目标目录统一。
const SELF = "dev-self";
const A = "dev-a";
const B = "dev-b";
const DIR = "C:\\Users\\x\\Docs";

describe("pastePlan(粘贴四态分派)", () => {
  it("paste_plan_routes_local_to_remote:源本机 → 目标远端 = upload", () => {
    const plan = pastePlan(clip(SELF, ["C:\\a.txt", "D:\\b"]), SELF, A, DIR);
    expect(plan).toEqual({
      kind: "upload",
      files: ["C:\\a.txt", "D:\\b"],
      to: A,
      dir: DIR,
      cut: false,
    });
  });

  it("paste_plan_routes_remote_to_local:源远端 → 目标本机 = download", () => {
    const plan = pastePlan(clip(A, ["/sd/a.txt"]), SELF, SELF, "C:\\Downloads");
    expect(plan).toEqual({
      kind: "download",
      device: A,
      paths: ["/sd/a.txt"],
      dir: "C:\\Downloads",
      cut: false,
    });
  });

  it("paste_plan_routes_remote_to_remote:远端 A → 远端 B = remote_copy(数据不经本机)", () => {
    const plan = pastePlan(clip(A, ["/sd/a"]), SELF, B, "/storage/0");
    expect(plan).toEqual({
      kind: "remote_copy",
      from: A,
      paths: ["/sd/a"],
      to: B,
      dir: "/storage/0",
      cut: false,
    });
  });

  it("paste_plan_routes_same_remote_device_copy:同一远端设备 = device_copy(ctrl copy/move,无网络传输)", () => {
    const plan = pastePlan(clip(A, ["/sd/a.txt"]), SELF, A, "/sd/b");
    expect(plan).toEqual({
      kind: "device_copy",
      device: A,
      paths: ["/sd/a.txt"],
      dir: "/sd/b",
      cut: false,
    });
  });

  it("paste_plan_routes_self_copy:源本机 → 目标本机 = local_copy", () => {
    const plan = pastePlan(clip(SELF, ["C:\\a.txt"]), SELF, SELF, "D:\\bak");
    expect(plan).toEqual({
      kind: "local_copy",
      paths: ["C:\\a.txt"],
      dir: "D:\\bak",
      cut: false,
    });
  });

  it("空内部剪贴板(引用 null 或 paths 空)→ none/empty(调用方转读系统剪贴板)", () => {
    expect(pastePlan(null, SELF, A, DIR)).toEqual({ kind: "none", reason: "empty" });
    expect(pastePlan(clip(A, []), SELF, A, DIR)).toEqual({ kind: "none", reason: "empty" });
  });
});

describe("dragPlan(窗口内拖放下发分派,与 pastePlan 同一纯函数)", () => {
  it("drag_plan_matches_paste_semantics:同设备拖 = 移动(cut=true),跨设备拖 = 复制(cut=false)", () => {
    // 同一台远端设备内拖 → device_copy 且 cut=true(移动)
    expect(dragPlan(A, ["/sd/a"], SELF, A, "/sd/b")).toEqual({
      kind: "device_copy",
      device: A,
      paths: ["/sd/a"],
      dir: "/sd/b",
      cut: true,
    });
    // 本机内拖 → local_copy 且 cut=true(移动)
    expect(dragPlan(SELF, ["C:\\a"], SELF, SELF, "C:\\b")).toEqual({
      kind: "local_copy",
      paths: ["C:\\a"],
      dir: "C:\\b",
      cut: true,
    });
    // 跨远端设备拖 → remote_copy 且 cut=false(复制,不删源)
    expect(dragPlan(A, ["/a"], SELF, B, "/b")).toEqual({
      kind: "remote_copy",
      from: A,
      paths: ["/a"],
      to: B,
      dir: "/b",
      cut: false,
    });
    // 本机 → 远端拖 = upload 复制(cut=false)
    expect(dragPlan(SELF, ["C:\\a"], SELF, A, "/b")).toEqual({
      kind: "upload",
      files: ["C:\\a"],
      to: A,
      dir: "/b",
      cut: false,
    });
    // 任何设备 → 本机目录 = 落盘 download(cut=false,不删源)
    expect(dragPlan(A, ["/a"], SELF, SELF, "C:\\Downloads")).toEqual({
      kind: "download",
      device: A,
      paths: ["/a"],
      dir: "C:\\Downloads",
      cut: false,
    });
  });
});

describe("cut_vs_copy_flag(剪切标志透传)", () => {
  it("cut=true 剪切板在五种可分派形态里全部透传 cut=true", () => {
    expect(pastePlan(clip(SELF, ["C:\\a"], true), SELF, A, "/b")).toMatchObject({ kind: "upload", cut: true });
    expect(pastePlan(clip(A, ["/a"], true), SELF, SELF, "C:\\d")).toMatchObject({ kind: "download", cut: true });
    expect(pastePlan(clip(A, ["/a"], true), SELF, B, "/b")).toMatchObject({ kind: "remote_copy", cut: true });
    expect(pastePlan(clip(A, ["/a"], true), SELF, A, "/b")).toMatchObject({ kind: "device_copy", cut: true });
    expect(pastePlan(clip(SELF, ["C:\\a"], true), SELF, SELF, "D:\\d")).toMatchObject({ kind: "local_copy", cut: true });
  });

  it("cut=false 复制板全程 cut=false(device_copy 走 copy RPC 而非 move)", () => {
    expect(pastePlan(clip(A, ["/a"], false), SELF, A, "/b")).toMatchObject({
      kind: "device_copy",
      cut: false,
    });
  });
});

describe("路径辅助(跨平台分隔符,device_copy 移动对拼目标用)", () => {
  it("baseName 取最后一段(反斜杠/正斜杠/混合),根形态为空串", () => {
    expect(baseName("C:\\Users\\x\\报告.txt")).toBe("报告.txt");
    expect(baseName("/storage/emulated/0/DCIM")).toBe("DCIM");
    expect(baseName("a/b\\c.txt")).toBe("c.txt");
    expect(baseName("C:\\")).toBe("");
    expect(baseName("/")).toBe("");
  });

  it("joinDirName 按目标目录的分隔符拼接,根形尾部不重复分隔符", () => {
    expect(joinDirName("C:\\Users\\x", "a.txt")).toBe("C:\\Users\\x\\a.txt");
    expect(joinDirName("C:\\", "a.txt")).toBe("C:\\a.txt");
    expect(joinDirName("/storage/0", "DCIM")).toBe("/storage/0/DCIM");
    expect(joinDirName("/", "DCIM")).toBe("/DCIM");
    // 已带尾分隔符的目录不再叠加
    expect(joinDirName("/storage/0/", "DCIM")).toBe("/storage/0/DCIM");
  });
});
