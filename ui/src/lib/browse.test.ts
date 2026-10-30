import { describe, expect, it } from "vitest";

import { codePointCmp } from "./collate";
import {
  applyListing,
  canEnterDir,
  filterEntries,
  fmtMtime,
  fmtSize,
  headerClick,
  sortEntries,
  type FsEntry,
  type Listing,
  type SortState,
} from "./browse";

/// 便捷构造条目:path 与 name 同值即可(排序/过滤只看 name);
/// extra 覆盖 size/mtime_ms 等字段(mtime/size 键测试用)。
function e(name: string, isDir = false, extra: Partial<FsEntry> = {}): FsEntry {
  return { name, path: name, size: 0, mtime_ms: 0, is_dir: isDir, ...extra };
}

describe("sortEntries", () => {
  const mixed = [e("b.txt"), e("z.dir", true), e("a.txt"), e("a.dir", true)];

  it("名称升序:目录恒在前,组内按注入 cmp 名称升序", () => {
    const out = sortEntries(mixed, { key: "name", dir: "asc" }, codePointCmp);
    expect(out.map((x) => x.name)).toEqual(["a.dir", "z.dir", "a.txt", "b.txt"]);
  });

  it("名称降序:目录仍恒在前,仅组内反序", () => {
    const out = sortEntries(mixed, { key: "name", dir: "desc" }, codePointCmp);
    expect(out.map((x) => x.name)).toEqual(["z.dir", "a.dir", "b.txt", "a.txt"]);
  });

  it("全目录 / 全文件时退化为纯名称序", () => {
    const dirs = [e("n2", true), e("n1", true), e("n3", true)];
    expect(sortEntries(dirs, { key: "name", dir: "asc" }, codePointCmp).map((x) => x.name)).toEqual(["n1", "n2", "n3"]);
    const files = [e("f2"), e("f1")];
    expect(sortEntries(files, { key: "name", dir: "desc" }, codePointCmp).map((x) => x.name)).toEqual(["f2", "f1"]);
  });

  it("同名条目保序(稳定排序)", () => {
    const input = [e("dup"), e("dup")];
    const out = sortEntries(input, { key: "name", dir: "asc" }, codePointCmp);
    expect(out[0]).toBe(input[0]);
    expect(out[1]).toBe(input[1]);
  });

  it("返回新数组,不改入参", () => {
    const input = [e("b"), e("a", true)];
    const snapshot = input.map((x) => ({ ...x }));
    sortEntries(input, { key: "name", dir: "asc" }, codePointCmp);
    expect(input).toEqual(snapshot);
  });

  it("四键×双向:目录恒在最前,任何键任何方向都不反转这一分区", () => {
    const keyed = [e("b.txt", false, { mtime_ms: 2, size: 20 }), e("d.dir", true, { mtime_ms: 1, size: 10 }), e("a.txt", false, { mtime_ms: 3, size: 30 })];
    for (const key of ["name", "mtime", "type", "size"] as const) {
      for (const dir of ["asc", "desc"] as const) {
        const out = sortEntries(keyed, { key, dir }, codePointCmp);
        expect(out.slice(0, 1).every((x) => x.is_dir), `${key}/${dir}`).toBe(true);
        expect(out.slice(1).some((x) => x.is_dir), `${key}/${dir}`).toBe(false);
      }
    }
  });

  it("mtime 键:数值比较(9 < 100,非字符串序),desc 双组各自反序", () => {
    // "100" < "9" 的字符串序是陷阱:数值比较应得 9 在前。
    const input = [e("t100", false, { mtime_ms: 100 }), e("t9", false, { mtime_ms: 9 }), e("d9", true, { mtime_ms: 9 }), e("d100", true, { mtime_ms: 100 })];
    expect(sortEntries(input, { key: "mtime", dir: "asc" }, codePointCmp).map((x) => x.name)).toEqual(["d9", "d100", "t9", "t100"]);
    expect(sortEntries(input, { key: "mtime", dir: "desc" }, codePointCmp).map((x) => x.name)).toEqual(["d100", "d9", "t100", "t9"]);
  });

  it("size 键:数值比较,目录也有 size(目录组内互比)", () => {
    const input = [e("big", false, { size: 100 }), e("small", false, { size: 9 }), e("dSmall", true, { size: 5 }), e("dBig", true, { size: 999 })];
    expect(sortEntries(input, { key: "size", dir: "asc" }, codePointCmp).map((x) => x.name)).toEqual(["dSmall", "dBig", "small", "big"]);
    expect(sortEntries(input, { key: "size", dir: "desc" }, codePointCmp).map((x) => x.name)).toEqual(["dBig", "dSmall", "big", "small"]);
  });

  it("type 键:主比类型显示名,同类型次键名称恒 asc(desc 只反转主键)", () => {
    // 显示名 codePoint 序:PDF 文档 < ZZZ 文件 < 文本文档。
    const input = [e("b.pdf"), e("a.pdf"), e("z.txt"), e("y.zzz")];
    expect(sortEntries(input, { key: "type", dir: "asc" }, codePointCmp).map((x) => x.name)).toEqual(["a.pdf", "b.pdf", "y.zzz", "z.txt"]);
    // desc 反转主键,但 PDF 组内 a.pdf 仍在 b.pdf 前(次键不随 dir 反转)。
    expect(sortEntries(input, { key: "type", dir: "desc" }, codePointCmp).map((x) => x.name)).toEqual(["z.txt", "y.zzz", "a.pdf", "b.pdf"]);
  });

  it("type 键:目录同为「文件夹」,组内名称 asc 不随 desc 反转", () => {
    const input = [e("z.txt"), e("b.dir", true), e("a.dir", true)];
    expect(sortEntries(input, { key: "type", dir: "desc" }, codePointCmp).map((x) => x.name)).toEqual(["a.dir", "b.dir", "z.txt"]);
  });
});

describe("headerClick", () => {
  it("同键:asc → desc 翻转", () => {
    expect(headerClick({ key: "name", dir: "asc" }, "name")).toEqual({ key: "name", dir: "desc" });
  });

  it("同键:desc → asc 翻转", () => {
    expect(headerClick({ key: "size", dir: "desc" }, "size")).toEqual({ key: "size", dir: "asc" });
  });

  it("新键:恒重置 asc(四键皆然,不继承当前方向)", () => {
    const cur: SortState = { key: "mtime", dir: "desc" };
    for (const key of ["name", "mtime", "type", "size"] as const) {
      if (key === cur.key) continue; // 同键翻转已由上两例覆盖
      expect(headerClick(cur, key), key).toEqual({ key, dir: "asc" });
    }
  });

  it("返回新对象,不改入参", () => {
    const cur: SortState = { key: "type", dir: "asc" };
    const out = headerClick(cur, "type");
    expect(out).not.toBe(cur);
    expect(cur).toEqual({ key: "type", dir: "asc" });
  });
});

describe("filterEntries", () => {
  const input = [e("Report.PDF"), e("photo.jpg"), e("notes.txt")];

  it("名称大小写不敏感子串匹配", () => {
    expect(filterEntries(input, "pdf").map((x) => x.name)).toEqual(["Report.PDF"]);
    expect(filterEntries(input, "PHOTO").map((x) => x.name)).toEqual(["photo.jpg"]);
    expect(filterEntries(input, "no").map((x) => x.name)).toEqual(["notes.txt"]);
  });

  it("无匹配 → 空数组;q 空 → 原样内容", () => {
    expect(filterEntries(input, "zzz")).toEqual([]);
    expect(filterEntries(input, "").map((x) => x.name)).toEqual(["Report.PDF", "photo.jpg", "notes.txt"]);
  });
});

describe("fmtSize", () => {
  it("目录 → 空串(大小列由调用方传 isDir)", () => {
    expect(fmtSize(0, true)).toBe("");
    expect(fmtSize(99999, true)).toBe("");
  });

  it("0 → \"0 B\";字节段整数、无小数", () => {
    expect(fmtSize(0, false)).toBe("0 B");
    expect(fmtSize(1, false)).toBe("1 B");
    expect(fmtSize(512, false)).toBe("512 B");
    expect(fmtSize(1023, false)).toBe("1023 B");
  });

  it("1024 进制、单位按阈值、一位小数不剪尾", () => {
    expect(fmtSize(1024, false)).toBe("1.0 KB");
    expect(fmtSize(1536, false)).toBe("1.5 KB");
    expect(fmtSize(1024 * 1024, false)).toBe("1.0 MB");
    expect(fmtSize(2 * 1024 * 1024, false)).toBe("2.0 MB");
    expect(fmtSize(1024 ** 3, false)).toBe("1.0 GB");
    expect(fmtSize(1024 ** 5, false)).toBe("1.0 PB");
  });
});

describe("fmtMtime", () => {
  it("ms=0 → 空串(无时间戳)", () => {
    expect(fmtMtime(0)).toBe("");
  });

  it("形状:YYYY/M/D H:MM(月/日/时不补零,分钟两位)", () => {
    // 固定 UTC 时刻只断言形状,不断言具体时钟值——跨时区环境成立。
    expect(fmtMtime(Date.UTC(2026, 9, 18, 1, 5))).toMatch(/^\d{4}\/\d{1,2}\/\d{1,2} \d{1,2}:\d{2}$/);
  });

  it("各分量与本地 Date 一致(期望值由同一时钟推导,环境无关)", () => {
    const ms = Date.UTC(2026, 9, 18, 13, 5, 30);
    const t = new Date(ms);
    const expected =
      `${t.getFullYear()}/${t.getMonth() + 1}/${t.getDate()}` +
      ` ${t.getHours()}:${String(t.getMinutes()).padStart(2, "0")}`;
    expect(fmtMtime(ms)).toBe(expected);
  });
});

describe("canEnterDir", () => {
  it("目标在线且未浏览离线设备 → 可进入", () => {
    expect(canEnterDir(true, false)).toBe(true);
  });

  it("目标离线 → 拒绝", () => {
    expect(canEnterDir(false, false)).toBe(false);
  });

  it("正在浏览离线设备 → 一律拒绝(离线阻断门控)", () => {
    expect(canEnterDir(true, true)).toBe(false);
    expect(canEnterDir(false, true)).toBe(false);
  });
});

describe("applyListing", () => {
  const listing: Listing = { entries: [e("a.txt")], truncated: false };

  it("响应 seq 旧于最新发出 seq → null(导航竞态守卫)", () => {
    expect(applyListing(1, 2, listing)).toBe(null);
  });

  it("最新 seq 透传;seq > latestSeq 视为非旧,不丢弃", () => {
    expect(applyListing(2, 2, listing)).toBe(listing);
    expect(applyListing(3, 2, listing)).toBe(listing);
  });

  it("listing 为 null(请求失败)原样透传", () => {
    expect(applyListing(2, 2, null)).toBe(null);
  });
});
