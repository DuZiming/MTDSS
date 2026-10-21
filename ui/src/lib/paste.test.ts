import { describe, expect, it } from "vitest";

import type { FsEntry } from "./browse";
import { pastePlan } from "./clipboard";
import type { SameDeviceArgs, SameDeviceSender } from "./paste";
import { planSameDeviceOp } from "./paste";

/// 便捷构造源/目标侧条目(FsEntry 全字段;冲突预扫只看 name/is_dir/size,
/// 执行材料另取 path)。
function entry(name: string, path: string, opts: Partial<FsEntry> = {}): FsEntry {
  return { name, path, size: 10, mtime_ms: 0, is_dir: false, ...opts };
}

/// 发送面捕集器:记录 (args, policy) 调用,供断言。
function recorder(): { calls: { args: SameDeviceArgs; policy: { skip: string[]; overwrite: string[] } }[]; send: SameDeviceSender } {
  const calls: { args: SameDeviceArgs; policy: { skip: string[]; overwrite: string[] } }[] = [];
  return { calls, send: (args, policy) => calls.push({ args, policy }) };
}

describe("planSameDeviceOp(同设备 copy/move 冲突分派)", () => {
  it("paste_plan_same_device_name_hit_yields_conflicts:同名文件对 → 冲突(scanConflicts 语义复用)", () => {
    const { calls, send } = recorder();
    const plan = planSameDeviceOp(
      "copy",
      [entry("a.txt", "C:\\src\\a.txt", { size: 100 })],
      "C:\\dst",
      [entry("a.txt", "C:\\dst\\a.txt", { size: 200, mtime_ms: 1234 })],
      send,
    );
    // 冲突行 = scanConflicts 语义:rel 为裸条目名,双侧元数据齐备。
    expect(plan.conflicts).toEqual([
      {
        rel: "a.txt",
        src_is_dir: false,
        src_size: 100,
        dst_is_dir: false,
        dst_size: 200,
        dst_mtime_ms: 1234,
      },
    ]);
    // 有冲突不直发:send 未被调用,执行交对话框裁决后的 exec。
    expect(calls).toEqual([]);
  });

  it("paste_plan_same_device_scan_semantics:类型不匹配冲突、目录对目录恒合并", () => {
    const { send } = recorder();
    // 文件 ↔ 目录(双向)→ 冲突;目录对目录 → 恒合并(不进对话框)。
    const plan = planSameDeviceOp(
      "copy",
      [
        entry("mix", "C:\\s\\mix"),
        entry("树", "C:\\s\\树", { is_dir: true }),
        entry("only.txt", "C:\\s\\only.txt"),
      ],
      "C:\\dst",
      [
        entry("mix", "C:\\dst\\mix", { is_dir: true }),
        entry("树", "C:\\dst\\树", { is_dir: true }),
      ],
      send,
    );
    expect(plan.conflicts.map((c) => c.rel)).toEqual(["mix"]);
    expect(plan.conflicts[0]?.src_is_dir).toBe(false);
    expect(plan.conflicts[0]?.dst_is_dir).toBe(true);
  });

  it("paste_plan_same_device_no_conflict_fires_exec_empty_policy:无同名 → 空 conflicts,exec 直发(策略空表)", () => {
    const { calls, send } = recorder();
    const plan = planSameDeviceOp(
      "copy",
      [entry("a.txt", "C:\\src\\a.txt"), entry("b.txt", "C:\\src\\b.txt")],
      "C:\\dst",
      [entry("c.txt", "C:\\dst\\c.txt")],
      send,
    );
    expect(plan.conflicts).toEqual([]);
    plan.exec({ skip: [], overwrite: [] });
    expect(calls).toEqual([
      { args: { op: "copy", paths: ["C:\\src\\a.txt", "C:\\src\\b.txt"], dir: "C:\\dst" }, policy: { skip: [], overwrite: [] } },
    ]);
  });

  it("paste_plan_same_device_policy_keys_join_dir:裁决 rel(裸名)按目标目录形态换算判定键(copy)", () => {
    const { calls, send } = recorder();
    // 目录含 "\" → "\" 拼接;不含 → "/";自带尾分隔符不重复——与引擎
    // policy_key / CLI join_remote 同规则(M4a-3 定稿契约)。
    const win = planSameDeviceOp("copy", [entry("a.txt", "C:\\s\\a.txt")], "C:\\dst", [entry("a.txt", "C:\\dst\\a.txt")], send);
    win.exec({ skip: ["a.txt"], overwrite: [] });
    expect(calls[0]?.policy).toEqual({ skip: ["C:\\dst\\a.txt"], overwrite: [] });

    const posix = planSameDeviceOp("copy", [entry("a.txt", "/s/a.txt")], "/dst", [entry("a.txt", "/dst/a.txt")], send);
    posix.exec({ skip: [], overwrite: ["a.txt"] });
    expect(calls[1]?.policy).toEqual({ skip: [], overwrite: ["/dst/a.txt"] });

    const rooted = planSameDeviceOp("copy", [entry("a.txt", "C:\\s\\a.txt")], "C:\\", [entry("a.txt", "C:\\a.txt")], send);
    rooted.exec({ skip: ["a.txt"], overwrite: ["a.txt"] });
    expect(calls[2]?.policy).toEqual({ skip: ["C:\\a.txt"], overwrite: ["C:\\a.txt"] });
  });

  it("paste_plan_same_device_move_pairs:move 材料为 rename 对(to = 目录拼源末段),判定键与 to 同形", () => {
    const { calls, send } = recorder();
    const plan = planSameDeviceOp(
      "move",
      [entry("a.txt", "C:\\src\\a.txt"), entry("sub", "C:\\src\\sub", { is_dir: true })],
      "C:\\dst",
      [entry("a.txt", "C:\\dst\\a.txt")],
      send,
    );
    // 目录对目录恒合并:仅 a.txt 进对话框;skip 落定 → 判定键 = to 串。
    expect(plan.conflicts.map((c) => c.rel)).toEqual(["a.txt"]);
    plan.exec({ skip: ["a.txt"], overwrite: [] });
    expect(calls).toEqual([
      {
        args: {
          op: "move",
          pairs: [
            { from: "C:\\src\\a.txt", to: "C:\\dst\\a.txt" },
            { from: "C:\\src\\sub", to: "C:\\dst\\sub" },
          ],
        },
        policy: { skip: ["C:\\dst\\a.txt"], overwrite: [] },
      },
    ]);
  });
});

describe("regression:本机同目录粘贴必达同机冲突预扫(Defect C)", () => {
  /// 本机设备 id(真实运行形态:树里「(本机)」行的 device_id 即 self)。
  const SELF = "dev-self";

  it("self_paste_same_dir_reaches_conflict_scan:pastePlan 须产出 device_copy,同名粘贴产出冲突行", () => {
    // 真实运行形态:dev_a(本机)浏览 C:\foo,选中 a.txt → Ctrl+C →
    // Ctrl+V 同目录。此前 pastePlan 把 本机→本机 派往 local_copy(无策略
    // 作业面),runTransfer 的 device_copy 预扫臂不可达 → exec 以空策略
    // 直发 → 引擎 legacy 自动改名 `a (1)`,对话框永不开。
    const plan = pastePlan(
      { device: SELF, paths: ["C:\\foo\\a.txt"], cut: false },
      SELF,
      SELF,
      "C:\\foo",
    );
    expect(plan.kind).toBe("device_copy");

    // device_copy 到达 runTransfer 后的链路(App.tsx 同构调用):
    // planSameDeviceOp 对目标目录列表(同一目录的 list,含源自身)预扫。
    const { calls, send } = recorder();
    const op = plan.kind === "device_copy" && plan.cut ? "move" : "copy";
    const sd = planSameDeviceOp(
      op,
      [entry("a.txt", "C:\\foo\\a.txt", { size: 1 })],
      "C:\\foo",
      [entry("a.txt", "C:\\foo\\a.txt", { size: 2, mtime_ms: 5 })],
      send,
    );
    expect(sd.conflicts.map((c) => c.rel)).toEqual(["a.txt"]);
    // 有冲突不得直发:裁决权在对话框。
    expect(calls).toEqual([]);
  });

  it("self_paste_move_same_dir:剪切粘贴同目录 → move 材料且判定键 = to 串", () => {
    const plan = pastePlan(
      { device: SELF, paths: ["C:\\foo\\a.txt"], cut: true },
      SELF,
      SELF,
      "C:\\foo",
    );
    expect(plan.kind).toBe("device_copy");
    const { calls, send } = recorder();
    const op = plan.kind === "device_copy" && plan.cut ? "move" : "copy";
    const sd = planSameDeviceOp(
      op,
      [entry("a.txt", "C:\\foo\\a.txt", { size: 1 })],
      "C:\\foo",
      [entry("a.txt", "C:\\foo\\a.txt", { size: 2, mtime_ms: 5 })],
      send,
    );
    sd.exec({ skip: [], overwrite: ["a.txt"] });
    expect(calls).toEqual([
      {
        args: { op: "move", pairs: [{ from: "C:\\foo\\a.txt", to: "C:\\foo\\a.txt" }] },
        policy: { skip: [], overwrite: ["C:\\foo\\a.txt"] },
      },
    ]);
  });
});
