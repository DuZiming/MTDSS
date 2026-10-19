import { describe, expect, it } from "vitest";

import { codePointCmp } from "./collate";
import { applyListing, canEnterDir, filterEntries, fmtMtime, fmtSize, sortEntries, type FsEntry, type Listing } from "./browse";

/// 便捷构造条目:path 与 name 同值即可(排序/过滤只看 name)。
function e(name: string, isDir = false): FsEntry {
  return { name, path: name, size: 0, mtime_ms: 0, is_dir: isDir };
}

describe("sortEntries", () => {
  const mixed = [e("b.txt"), e("z.dir", true), e("a.txt"), e("a.dir", true)];

  it("升序:目录恒在前,组内按注入 cmp 名称升序", () => {
    const out = sortEntries(mixed, "asc", codePointCmp);
    expect(out.map((x) => x.name)).toEqual(["a.dir", "z.dir", "a.txt", "b.txt"]);
  });

  it("降序:目录仍恒在前,仅组内反序", () => {
    const out = sortEntries(mixed, "desc", codePointCmp);
    expect(out.map((x) => x.name)).toEqual(["z.dir", "a.dir", "b.txt", "a.txt"]);
  });

  it("全目录 / 全文件时退化为纯名称序", () => {
    const dirs = [e("n2", true), e("n1", true), e("n3", true)];
    expect(sortEntries(dirs, "asc", codePointCmp).map((x) => x.name)).toEqual(["n1", "n2", "n3"]);
    const files = [e("f2"), e("f1")];
    expect(sortEntries(files, "desc", codePointCmp).map((x) => x.name)).toEqual(["f2", "f1"]);
  });

  it("同名条目保序(稳定排序)", () => {
    const input = [e("dup"), e("dup")];
    const out = sortEntries(input, "asc", codePointCmp);
    expect(out[0]).toBe(input[0]);
    expect(out[1]).toBe(input[1]);
  });

  it("返回新数组,不改入参", () => {
    const input = [e("b"), e("a", true)];
    const snapshot = input.map((x) => ({ ...x }));
    sortEntries(input, "asc", codePointCmp);
    expect(input).toEqual(snapshot);
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
