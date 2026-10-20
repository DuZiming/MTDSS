import { describe, expect, it } from "vitest";

import type { FsEntry } from "./browse";
import {
  applyAllDecisions,
  groupConflicts,
  policyFromDecisions,
  scanConflicts,
  type Conflict,
  type ConflictSource,
} from "./conflict";

/// 便捷构造目标侧条目(字段面与 FsEntry 对齐,预扫只用 name/is_dir/size)。
function dst(name: string, isDir = false, size = 0, mtime = 0): FsEntry {
  return { name, path: name, size, mtime_ms: mtime, is_dir: isDir };
}

/// 便捷构造源侧条目。
function src(name: string, isDir = false, size = 0): ConflictSource {
  return { name, is_dir: isDir, size };
}

describe("scanConflicts(预扫:名字相等比对,目录恒合并)", () => {
  it("文件对文件 = 冲突;类型不匹配(双向)= 冲突;目录对目录 = 恒合并不进对话框", () => {
    const sources = [
      src("a.txt", false, 100), // 文件对文件 → 冲突
      src("pics", true, 0), // 目录对目录 → 合并(无冲突)
      src("b.bin", false, 1), // 文件对目录 → 类型不匹配 → 冲突
      src("c", true, 0), // 目录对文件 → 类型不匹配 → 冲突
      src("fresh.txt", false, 5), // 目标无同名 → 无冲突
    ];
    const dstEntries = [
      dst("a.txt", false, 200, 1729000000000),
      dst("pics", true, 0, 1729000000001),
      dst("b.bin", true, 0, 1729000000002),
      dst("c", false, 7, 1729000000003),
    ];
    const out = scanConflicts(sources, dstEntries);
    expect(out.map((c) => c.rel)).toEqual(["a.txt", "b.bin", "c"]);
    // 冲突行携带双侧元数据(name/is_dir/size/目标 mtime;目录 size 恒 0)
    expect(out[0]).toEqual({
      rel: "a.txt",
      src_is_dir: false,
      src_size: 100,
      dst_is_dir: false,
      dst_size: 200,
      dst_mtime_ms: 1729000000000,
    });
  });

  it("源序稳定:输出按源序,目标侧无同名 → 空表", () => {
    expect(scanConflicts([src("x"), src("y")], [dst("z")])).toEqual([]);
    const a: Conflict[] = scanConflicts(
      [src("z", false, 1), src("x", false, 2)],
      [dst("x", false, 3, 1), dst("z", false, 4, 2)],
    );
    expect(a.map((c) => c.rel)).toEqual(["z", "x"]);
  });
});

describe("groupConflicts(单个 vs 批量)", () => {
  it("group_conflicts_single_vs_batch:1 条 → single;≥2 条 → 批量,rows 原样保序", () => {
    const one: Conflict[] = [
      { rel: "a", src_is_dir: false, src_size: 1, dst_is_dir: false, dst_size: 2, dst_mtime_ms: 3 },
    ];
    expect(groupConflicts(one)).toEqual({ single: true, rows: one });
    const two: Conflict[] = [one[0], { ...one[0], rel: "b" }];
    const g = groupConflicts(two);
    expect(g.single).toBe(false);
    expect(g.rows).toEqual(two);
    expect(groupConflicts([])).toEqual({ single: false, rows: [] });
  });
});

describe("applyAllDecisions(全部键只作用剩余未决项)", () => {
  const conflicts: Conflict[] = [
    { rel: "a", src_is_dir: false, src_size: 1, dst_is_dir: false, dst_size: 2, dst_mtime_ms: 3 },
    { rel: "b", src_is_dir: false, src_size: 1, dst_is_dir: false, dst_size: 2, dst_mtime_ms: 3 },
    { rel: "c", src_is_dir: false, src_size: 1, dst_is_dir: false, dst_size: 2, dst_mtime_ms: 3 },
  ];

  it("apply_all_decisions:a 已决 skip,全部覆盖只把 b/c 翻成 overwrite,a 保持 skip", () => {
    const out = applyAllDecisions({ a: "skip" }, conflicts, "overwrite");
    expect(out).toEqual({ a: "skip", b: "overwrite", c: "overwrite" });
  });

  it("apply_all_decisions:反向同理(a 已决 overwrite,全部跳过不动 a)", () => {
    const out = applyAllDecisions({ a: "overwrite" }, conflicts, "skip");
    expect(out).toEqual({ a: "overwrite", b: "skip", c: "skip" });
  });

  it("apply_all_decisions:无已决项时全量生效;返回新对象不改入参", () => {
    const before: Record<string, "skip" | "overwrite"> = { a: "skip" };
    const out = applyAllDecisions(before, conflicts, "skip");
    expect(out).toEqual({ a: "skip", b: "skip", c: "skip" });
    expect(before).toEqual({ a: "skip" });
  });
});

describe("policyFromDecisions(决策落定 → skip/overwrite 名单)", () => {
  it("已决项按决投影;未决项不进任何名单(调用方门控「全部决完才放行」)", () => {
    const conflicts: Conflict[] = [
      { rel: "a", src_is_dir: false, src_size: 1, dst_is_dir: false, dst_size: 2, dst_mtime_ms: 3 },
      { rel: "b", src_is_dir: false, src_size: 1, dst_is_dir: false, dst_size: 2, dst_mtime_ms: 3 },
      { rel: "c", src_is_dir: false, src_size: 1, dst_is_dir: false, dst_size: 2, dst_mtime_ms: 3 },
    ];
    const policy = policyFromDecisions(conflicts, { a: "skip", b: "overwrite" });
    expect(policy).toEqual({ skip: ["a"], overwrite: ["b"] });
    expect(policyFromDecisions(conflicts, {})).toEqual({ skip: [], overwrite: [] });
  });
});
