import { describe, expect, it } from "vitest";

import type { FsEntry, SortState } from "./browse";
import { codePointCmp } from "./collate";
import { memberKey, type FavMember } from "./favorites";
import {
  groupRowsByDevice,
  groupedSections,
  type DragSourceSel,
  type MemberState,
  memberStateUpsert,
  type MixedRow,
  mixedRows,
  offlineMembers,
  planMultiSourceDrop,
  runGroupsWithConflictPause,
  unavailableCount,
} from "./mixedview";

const cmp = codePointCmp;
const ascName: SortState = { key: "name", dir: "asc" };

/// 便捷构造成员。
function m(device: string, path: string): FavMember {
  return { device, path };
}

/// 便捷构造条目(默认文件:size 1、mtime 0)。
function e(name: string, path: string, is_dir = false): FsEntry {
  return { name, path, size: 1, mtime_ms: 0, is_dir };
}

describe("offlineMembers 快照分桶", () => {
  it("在线成员 → pending(在途),离线成员 → offline;entries 恒空,序=成员添加序", () => {
    const out = offlineMembers([m("d1", "C:\\a"), m("d2", "/b")], (d) => d === "d1");
    expect(out).toEqual([
      { member: m("d1", "C:\\a"), status: "pending", entries: [] },
      { member: m("d2", "/b"), status: "offline", entries: [] },
    ]);
  });

  it("空成员清单 → 空 states", () => {
    expect(offlineMembers([], () => true)).toEqual([]);
  });
});

describe("memberStateUpsert", () => {
  it("pending → ok 原位替换(保成员添加序),其余成员不动", () => {
    const seed = offlineMembers([m("d1", "C:\\a"), m("d2", "/b")], () => true);
    const out = memberStateUpsert(seed, m("d2", "/b"), "ok", [e("x.txt", "/b/x.txt")]);
    expect(out.map((s) => s.member.device)).toEqual(["d1", "d2"]);
    expect(out[0].status).toBe("pending");
    expect(out[1]).toEqual({ member: m("d2", "/b"), status: "ok", entries: [e("x.txt", "/b/x.txt")] });
  });

  it("不在 states 的成员 → 尾部追加(新添加序)", () => {
    const seed = offlineMembers([m("d1", "C:\\a")], () => true);
    const out = memberStateUpsert(seed, m("d9", "/z"), "ok", []);
    expect(out.map((s) => s.member.device)).toEqual(["d1", "d9"]);
  });

  it("failed 桶(Review Focus #3):list 失败后 upsert failed 入桶", () => {
    const seed = offlineMembers([m("d1", "C:\\a")], () => true);
    const out = memberStateUpsert(seed, m("d1", "C:\\a"), "failed", []);
    expect(out[0]).toEqual({ member: m("d1", "C:\\a"), status: "failed", entries: [] });
    expect(unavailableCount(out)).toBe(1);
  });

  it("纯函数:不改入参数组", () => {
    const seed = offlineMembers([m("d1", "C:\\a")], () => true);
    const snapshot = JSON.stringify(seed);
    memberStateUpsert(seed, m("d1", "C:\\a"), "ok", [e("x.txt", "C:\\a\\x.txt")]);
    expect(JSON.stringify(seed)).toBe(snapshot);
  });
});

describe("mixedRows 渐进合并(spec §5.2)", () => {
  /// 两成员:先 A(d1)后 B(d2),应答逐个并入、逐次重排。
  const mA = m("d1", "C:\\A");
  const mB = m("d2", "C:\\B");
  const entriesA = [e("zeta", "C:\\A\\zeta", true), e("beta.txt", "C:\\A\\beta.txt")];
  const entriesB = [e("alpha", "C:\\B\\alpha", true), e("aardvark.txt", "C:\\B\\aardvark.txt")];

  it("只有 A 到达:仅 A 的行,B(pending)不出行;目录恒前", () => {
    const s1 = memberStateUpsert(offlineMembers([mA, mB], () => true), mA, "ok", entriesA);
    const rows = mixedRows(s1, ascName, cmp);
    expect(rows.map((r) => r.entry.name)).toEqual(["zeta", "beta.txt"]);
    expect(rows.every((r) => r.device === "d1")).toBe(true);
  });

  it("B 也到达:全量重排,目录恒前、文件按名升序", () => {
    const s1 = memberStateUpsert(offlineMembers([mA, mB], () => true), mA, "ok", entriesA);
    const s2 = memberStateUpsert(s1, mB, "ok", entriesB);
    const rows = mixedRows(s2, ascName, cmp);
    expect(rows.map((r) => r.entry.name)).toEqual(["alpha", "zeta", "aardvark.txt", "beta.txt"]);
  });

  it("排序稳定:同名并列保持成员添加序(先并入的成员在前)", () => {
    const dupA = [...entriesA, e("dup.txt", "C:\\A\\dup.txt")];
    const dupB = [...entriesB, e("dup.txt", "C:\\B\\dup.txt")];
    const s1 = memberStateUpsert(offlineMembers([mA, mB], () => true), mA, "ok", dupA);
    const s2 = memberStateUpsert(s1, mB, "ok", dupB);
    const dupRows = mixedRows(s2, ascName, cmp).filter((r) => r.entry.name === "dup.txt");
    expect(dupRows.map((r) => r.device)).toEqual(["d1", "d2"]);
  });

  it("离线 / 失败成员不入混排行", () => {
    const s1 = memberStateUpsert(offlineMembers([mA, mB], (d) => d === "d1"), mA, "ok", entriesA);
    expect(mixedRows(s1, ascName, cmp).map((r) => r.device)).toEqual(["d1", "d1"]);
    const s2 = memberStateUpsert(offlineMembers([mA, mB], () => true), mB, "failed", []);
    expect(mixedRows(s2, ascName, cmp)).toEqual([]);
  });

  it("纯函数:不改入参 states(合并重排不动原条目)", () => {
    const s1 = memberStateUpsert(offlineMembers([mA, mB], () => true), mA, "ok", entriesA);
    const snapshot = JSON.stringify(s1);
    mixedRows(s1, ascName, cmp);
    groupedSections(s1, ascName, cmp);
    expect(JSON.stringify(s1)).toBe(snapshot);
  });
});

describe("同路径跨源两行并存(Review Focus #2)", () => {
  it("两成员各含同 path 文件 → 两行两 key;并列稳定=成员添加序", () => {
    const mA = m("d1", "C:\\docs");
    const mB = m("d2", "C:\\docs");
    const s = memberStateUpsert(
      memberStateUpsert(offlineMembers([mA, mB], () => true), mA, "ok", [e("notes.txt", "C:\\docs\\notes.txt")]),
      mB,
      "ok",
      [e("notes.txt", "C:\\docs\\notes.txt")],
    );
    const rows = mixedRows(s, ascName, cmp);
    expect(rows).toHaveLength(2);
    expect(rows[0].key).toBe(memberKey({ device: "d1", path: "C:\\docs\\notes.txt" }));
    expect(rows[1].key).toBe(memberKey({ device: "d2", path: "C:\\docs\\notes.txt" }));
    expect(new Set(rows.map((r) => r.key)).size).toBe(2);
    expect(rows.map((r) => r.device)).toEqual(["d1", "d2"]);
  });
});

describe("groupedSections 分组节", () => {
  it("节序=成员添加序(states 序);离线/失败节 rows 空;ok 节内=当前排序", () => {
    const seed = offlineMembers([m("d1", "C:\\A"), m("d2", "C:\\B"), m("d3", "C:\\C")], (d) => d !== "d2");
    const s1 = memberStateUpsert(
      seed,
      m("d1", "C:\\A"),
      "ok",
      [e("beta.txt", "C:\\A\\beta.txt"), e("zeta", "C:\\A\\zeta", true)],
    );
    const s2 = memberStateUpsert(s1, m("d3", "C:\\C"), "failed", []);
    const secs = groupedSections(s2, ascName, cmp);
    expect(secs.map((sec) => sec.member.device)).toEqual(["d1", "d2", "d3"]);
    expect(secs[0].rows.map((r) => r.entry.name)).toEqual(["zeta", "beta.txt"]); // 目录恒前
    expect(secs[1].rows).toEqual([]);
    expect(secs[2].rows).toEqual([]);
    expect(secs[1].state.status).toBe("offline");
    expect(secs[2].state.status).toBe("failed");
  });
});

describe("unavailableCount", () => {
  it("只计 offline + failed;ok / pending 不计;空 states → 0", () => {
    const states: MemberState[] = [
      { member: m("d1", "C:\\a"), status: "ok", entries: [] },
      { member: m("d2", "C:\\b"), status: "pending", entries: [] },
      { member: m("d3", "C:\\c"), status: "offline", entries: [] },
      { member: m("d4", "C:\\d"), status: "failed", entries: [] },
      { member: m("d5", "C:\\e"), status: "offline", entries: [] },
    ];
    expect(unavailableCount(states)).toBe(3);
    expect(unavailableCount([])).toBe(0);
  });
});

describe("groupRowsByDevice 跨源分组", () => {
  /// 渲染序: d1/a.txt、d2/x.bin、d1/b.txt(两设备交错)。
  const rows: MixedRow[] = [
    { entry: e("a.txt", "C:\\a\\a.txt"), device: "d1", key: "d1\u0000C:\\a\\a.txt" },
    { entry: e("x.bin", "/b/x.bin"), device: "d2", key: "d2\u0000/b/x.bin" },
    { entry: e("b.txt", "C:\\a\\b.txt"), device: "d1", key: "d1\u0000C:\\a\\b.txt" },
  ];

  it("3 行 2 设备全选 → 2 组;组序=设备首次出现序(渲染序),组内保持渲染序", () => {
    const all = new Set(rows.map((r) => r.key));
    expect(groupRowsByDevice(rows, all)).toEqual([
      { device: "d1", entries: [rows[0].entry, rows[2].entry] },
      { device: "d2", entries: [rows[1].entry] },
    ]);
  });

  it("部分选中只出选中行;全不选 → 空组集", () => {
    expect(groupRowsByDevice(rows, new Set([rows[1].key]))).toEqual([{ device: "d2", entries: [rows[1].entry] }]);
    expect(groupRowsByDevice(rows, new Set())).toEqual([]);
  });
});

describe("runGroupsWithConflictPause 分组推进(T11 评审 Important #1:冲突挂起即停)", () => {
  /// 直驱驱动器:run 记录发起序,按 parkAt 在该组完成后置挂起标记。
  async function drive(count: number, parkAt: number | null) {
    let parkFlag = false;
    const ran: number[] = [];
    const out = await runGroupsWithConflictPause(
      Array.from({ length: count }, (_, i) => i),
      async (g) => {
        ran.push(g);
        if (g === parkAt) parkFlag = true;
      },
      () => parkFlag,
    );
    return { ran, out };
  }

  it("无挂起:全组直发,rest=0", async () => {
    const { ran, out } = await drive(3, null);
    expect(ran).toEqual([0, 1, 2]);
    expect(out).toEqual({ initiated: 3, rest: 0 });
  });

  it("中组挂起:后续组不再发起(静默丢组→可见),rest 计未发起数", async () => {
    const { ran, out } = await drive(3, 1);
    expect(ran).toEqual([0, 1]); // 组 2 未发起
    expect(out).toEqual({ initiated: 1, rest: 1 });
  });

  it("末组挂起:前面组已直发,其后无组 → rest=0(无静默丢组,不提示)", async () => {
    const { ran, out } = await drive(2, 1);
    expect(ran).toEqual([0, 1]);
    expect(out).toEqual({ initiated: 1, rest: 0 });
  });

  it("单组挂起:rest=0(单组冲突原路径不受影响)", async () => {
    const { out } = await drive(1, 0);
    expect(out).toEqual({ initiated: 0, rest: 0 });
  });
});

describe("planMultiSourceDrop(V3-3 Task 12:多源落点 fan-out 规划)", () => {
  const src = (device: string, srcDir: string | null, ...names: string[]): DragSourceSel => ({
    device,
    srcDir,
    entries: names.map((n) => e(n, `${srcDir ?? "C:\\x"}\\${n}`)),
  });

  it("单源(dir 位形态)→ 1 组计划,plan 与 dragPlan 逐字同形(自设备→远端=upload)", () => {
    const out = planMultiSourceDrop([src("self", "C:\\a", "f.txt")], "self", "d2", "C:\\b");
    expect(out).toHaveLength(1);
    expect(out[0].plan).toEqual({ kind: "upload", files: ["C:\\a\\f.txt"], to: "d2", dir: "C:\\b", cut: false });
    expect(out[0].source.entries.map((x) => x.name)).toEqual(["f.txt"]);
  });

  it("多源 → 逐组计划,组序=入参序(自设备 upload / 远端 remote_copy 各按判定)", () => {
    const out = planMultiSourceDrop(
      [src("self", "C:\\mine", "a.txt"), src("d1", "C:\\d1", "c.bin")],
      "self",
      "d2",
      "C:\\b",
    );
    expect(out.map((p) => p.plan.kind)).toEqual(["upload", "remote_copy"]);
    expect(out[1].plan).toEqual({ kind: "remote_copy", from: "d1", paths: ["C:\\d1\\c.bin"], to: "d2", dir: "C:\\b", cut: false });
  });

  it("原地放置组跳过(group.device===targetDevice && group.srcDir===targetDir),余组照出", () => {
    const out = planMultiSourceDrop(
      [src("d2", "C:\\b", "same.txt"), src("d1", "C:\\d1", "keep.bin")],
      "self",
      "d2",
      "C:\\b",
    );
    expect(out).toHaveLength(1);
    expect(out[0].source.device).toBe("d1");
  });

  it("全组原地放置 → 空计划集(单源原地 no-op 静默同义)", () => {
    expect(planMultiSourceDrop([src("d2", "C:\\b", "x.txt")], "self", "d2", "C:\\b")).toEqual([]);
  });

  it("fav 位源组(srcDir=null)不做原地判定;空条目组 → plan none 被剔除", () => {
    expect(planMultiSourceDrop([src("d2", null, "y.txt")], "self", "d2", "C:\\b").map((p) => p.plan.kind)).toEqual(["device_copy"]);
    expect(planMultiSourceDrop([{ device: "d1", srcDir: null, entries: [] }], "self", "d2", "C:\\b")).toEqual([]);
  });

  it("纯函数:不改入参", () => {
    const sources = [src("self", "C:\\a", "f.txt")];
    planMultiSourceDrop(sources, "self", "d2", "C:\\b");
    expect(sources[0].entries).toHaveLength(1);
  });
});
