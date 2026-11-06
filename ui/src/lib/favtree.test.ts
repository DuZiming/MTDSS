/// favtree 纯投影钉测试(V3-3 返工二 §14.1):并集排序(目录恒前 + 跨源
/// 按名交织)、同名并列保持成员添加序(sortEntries 稳定)、复合行键
/// (device\0path,同路径跨源两行并存)、离线/失败成员缺席计数。
import { describe, expect, it } from "vitest";

import type { FsEntry } from "./browse";
import { zhCmp } from "./collate";
import { absentMemberCount, FAV_TREE_SORT, mergeMemberListings } from "./favtree";
import type { MemberState, MemberStatus } from "./mixedview";

const entry = (name: string, path: string, isDir: boolean): FsEntry => ({
  name,
  path,
  size: isDir ? 0 : 12,
  mtime_ms: 0,
  is_dir: isDir,
});

/// 单成员状态构造器(entries 仅 ok 态承载;其余态恒空,与 favfetch 落态同形)。
const st = (device: string, path: string, status: MemberStatus, entries: FsEntry[]): MemberState => ({
  member: { device, path },
  status,
  entries,
});

describe("mergeMemberListings(§14.1 成员内容合并)", () => {
  it("目录恒前组内按名,文件组跨源按名交织(device 不同名条目穿插)", () => {
    // 成员序:dev-a 先、dev-b 后;目录组 z(全名比 m 大)在前?——目录组内
    // 按名:m < z。文件组:a.txt(dev-b)插在 b.txt(dev-a)前 = 跨源交织。
    const states = [
      st("dev-a", "C:\\P", "ok", [entry("b.txt", "C:\\P\\b.txt", false), entry("z", "C:\\P\\z", true)]),
      st("dev-b", "/srv", "ok", [entry("a.txt", "/srv/a.txt", false), entry("m", "/srv/m", true)]),
    ];
    const rows = mergeMemberListings(states, zhCmp());
    expect(rows.map((r) => r.entry.name)).toEqual(["m", "z", "a.txt", "b.txt"]);
    expect(rows.map((r) => r.device)).toEqual(["dev-b", "dev-a", "dev-b", "dev-a"]);
    expect(FAV_TREE_SORT).toEqual({ key: "name", dir: "asc" });
  });

  it("同名并列保持成员添加序(sortEntries 稳定;同路径跨源两行并存)", () => {
    const states = [
      st("dev-a", "C:\\P", "ok", [entry("同名.txt", "C:\\P\\同名.txt", false)]),
      st("dev-b", "/srv", "ok", [entry("同名.txt", "/srv/同名.txt", false)]),
    ];
    const rows = mergeMemberListings(states, zhCmp());
    expect(rows).toHaveLength(2);
    // 复合行键 device\0path:同 path 名跨源两行 key 必不同
    expect(rows[0]).toMatchObject({ device: "dev-a", key: "dev-a\u0000C:\\P\\同名.txt" });
    expect(rows[1]).toMatchObject({ device: "dev-b", key: "dev-b\u0000/srv/同名.txt" });
  });

  it("离线/失败成员条目缺席,ok/pending 照常并入", () => {
    const states = [
      st("dev-a", "C:\\P", "ok", [entry("a.txt", "C:\\P\\a.txt", false)]),
      st("dev-off", "/x", "offline", []),
      st("dev-fail", "/y", "failed", []),
      st("dev-pend", "/z", "pending", []),
    ];
    const rows = mergeMemberListings(states, zhCmp());
    expect(rows.map((r) => r.device)).toEqual(["dev-a"]);
  });

  it("absentMemberCount:offline + failed 计入,ok/pending 不计", () => {
    const states = [
      st("dev-a", "C:\\P", "ok", []),
      st("dev-off", "/x", "offline", []),
      st("dev-fail", "/y", "failed", []),
      st("dev-fail2", "/w", "failed", []),
      st("dev-pend", "/z", "pending", []),
    ];
    expect(absentMemberCount(states)).toBe(3);
    expect(absentMemberCount([st("dev-a", "C:\\P", "ok", [])])).toBe(0);
  });
});
