import { describe, expect, it } from "vitest";

import { type Loc, type NavState, joinRel, navBack, navForward, navGo, locAtSegment, parentLoc, parentPath, pathSegments, relUnderBase } from "./nav";

/// 便捷构造目录位。
function dir(dev: string, path: string): Loc {
  return { kind: "dir", device: dev, path };
}

/// 便捷构造快捷命名空间位(V3-3 返工 §13.6)。
function favdir(favId: string, dev: string, base: string, rel: string): Loc {
  return { kind: "favdir", favId, device: dev, base, rel };
}

const DEV: Loc = { kind: "devices" };
const A = dir("dev-1", "C:\\");
const B = dir("dev-1", "C:\\docs");
const C = dir("dev-1", "C:\\docs\\src");
const F1: Loc = { kind: "fav", id: "f1" };
const FD_ROOT = favdir("f1", "dev-1", "C:\\docs", "");
const FD_SUB = favdir("f1", "dev-1", "C:\\docs", "src");
const FD_DEEP = favdir("f1", "dev-1", "C:\\docs", "src\\out");

describe("parentPath", () => {
  it("Windows 反斜杠路径逐级上溯", () => {
    expect(parentPath("C:\\a\\b")).toBe("C:\\a");
    expect(parentPath("C:\\a")).toBe("C:\\"); // 恰两段父为盘根(规范形带尾分隔)
    expect(parentPath("C:\\a\\b\\")).toBe("C:\\a"); // 尾分隔符等价无尾分隔
  });

  it("Windows 盘根无父 → null", () => {
    expect(parentPath("C:\\")).toBe(null);
    expect(parentPath("C:")).toBe(null); // 裸盘符等同盘根
  });

  it("POSIX 绝对路径逐级上溯,根 → null", () => {
    expect(parentPath("/a/b")).toBe("/a");
    expect(parentPath("/a")).toBe("/");
    expect(parentPath("/a/b/")).toBe("/a");
    expect(parentPath("/")).toBe(null);
  });
});

describe("pathSegments", () => {
  it("Windows:盘符为第一段", () => {
    expect(pathSegments("C:\\a\\b")).toEqual(["C:", "a", "b"]);
    expect(pathSegments("C:\\")).toEqual(["C:"]);
    expect(pathSegments("C:\\a\\")).toEqual(["C:", "a"]);
  });

  it("POSIX:根独立成段", () => {
    expect(pathSegments("/a/b")).toEqual(["/", "a", "b"]);
    expect(pathSegments("/")).toEqual(["/"]);
    expect(pathSegments("/a/")).toEqual(["/", "a"]);
  });
});

describe("locAtSegment", () => {
  const DEV_ID = "dev-1";

  it("Windows 路径第 k 段 → 对应深度的目录位", () => {
    expect(locAtSegment(DEV_ID, "C:\\a\\b", 1)).toEqual(dir(DEV_ID, "C:\\"));
    expect(locAtSegment(DEV_ID, "C:\\a\\b", 2)).toEqual(dir(DEV_ID, "C:\\a"));
    expect(locAtSegment(DEV_ID, "C:\\a\\b", 3)).toEqual(dir(DEV_ID, "C:\\a\\b"));
  });

  it("k 超出段数截到最深;k<=0 → 设备层", () => {
    expect(locAtSegment(DEV_ID, "C:\\a\\b", 9)).toEqual(dir(DEV_ID, "C:\\a\\b"));
    expect(locAtSegment(DEV_ID, "C:\\a\\b", 0)).toEqual(DEV);
    expect(locAtSegment(DEV_ID, "/a/b", -1)).toEqual(DEV);
  });

  it("POSIX 路径第 1 段即根", () => {
    expect(locAtSegment(DEV_ID, "/a/b", 1)).toEqual(dir(DEV_ID, "/"));
    expect(locAtSegment(DEV_ID, "/a/b", 2)).toEqual(dir(DEV_ID, "/a"));
    expect(locAtSegment(DEV_ID, "/a/b", 3)).toEqual(dir(DEV_ID, "/a/b"));
  });
});

describe("parentLoc", () => {
  it("目录位逐级上溯:盘根 / POSIX 根 → 设备层", () => {
    expect(parentLoc({ kind: "devices" })).toEqual(DEV);
    expect(parentLoc(A)).toEqual(DEV); // "C:\" 为根
    expect(parentLoc(dir("dev-1", "/"))).toEqual(DEV); // "/" 为根
  });

  it("非根目录 → 父目录位(设备 id 保留)", () => {
    expect(parentLoc(B)).toEqual(dir("dev-1", "C:\\"));
    expect(parentLoc(C)).toEqual(B);
    expect(parentLoc(dir("dev-1", "/a"))).toEqual(dir("dev-1", "/"));
  });

  it("快捷目录位上级为设备层(V3-3 裁量 #5)", () => {
    expect(parentLoc(F1)).toEqual(DEV);
  });

  it("favdir rel 逐级去段;顶级(rel=\"\")回该快捷混合视图(返工 §13.6)", () => {
    expect(parentLoc(FD_DEEP)).toEqual(FD_SUB);
    expect(parentLoc(FD_SUB)).toEqual(FD_ROOT);
    expect(parentLoc(FD_ROOT)).toEqual(F1); // 成员根的上级=fav 混合视图
    // POSIX base/rel 同规则(分隔符随 rel 探测)
    const pRoot = favdir("f1", "dev-2", "/srv", "");
    const pSub = favdir("f1", "dev-2", "/srv", "data/zz");
    expect(parentLoc(pSub)).toEqual(favdir("f1", "dev-2", "/srv", "data"));
    expect(parentLoc(pRoot)).toEqual(F1);
  });
});

describe("joinRel / relUnderBase(返工 §13.6 favdir 拼段/反推)", () => {
  it("joinRel:分隔符随 base,base 带尾分隔符直接接", () => {
    expect(joinRel("C:\\docs", "src")).toBe("C:\\docs\\src");
    expect(joinRel("C:\\docs", "src\\out")).toBe("C:\\docs\\src\\out");
    expect(joinRel("C:\\", "docs")).toBe("C:\\docs"); // 盘根自带尾分隔
    expect(joinRel("/srv", "data")).toBe("/srv/data");
    expect(joinRel("/", "data")).toBe("/data"); // POSIX 根
    expect(joinRel("C:\\docs", "")).toBe("C:\\docs"); // rel="" = 成员根本身
  });

  it("relUnderBase:child=base → \"\";子路径剥前缀;不在之下 → null", () => {
    expect(relUnderBase("C:\\docs", "C:\\docs")).toBe("");
    expect(relUnderBase("C:\\docs", "C:\\docs\\src")).toBe("src");
    expect(relUnderBase("C:\\docs", "C:\\docs\\src\\out")).toBe("src\\out");
    expect(relUnderBase("C:\\docs\\", "C:\\docs\\src")).toBe("src"); // base 带尾分隔
    expect(relUnderBase("C:\\docs", "C:\\other")).toBe(null);
    expect(relUnderBase("C:\\docs", "C:\\docsx")).toBe(null); // 前缀非段边界
    expect(relUnderBase("/srv", "/srv/data")).toBe("data");
    expect(relUnderBase("/", "/data")).toBe("data");
    expect(relUnderBase("/", "/")).toBe(""); // child=base(POSIX 根)
  });
});

describe("navGo", () => {
  it("current 压入 back,current 换为 next,fwd 清空", () => {
    const s1 = navGo({ back: [], fwd: [], current: A }, B);
    expect(s1.back).toEqual([A]);
    expect(s1.current).toEqual(B);
    expect(s1.fwd).toEqual([]);
    const s2 = navGo(s1, C);
    expect(s2.back).toEqual([A, B]);
    expect(s2.current).toEqual(C);
  });

  it("重复点击同一位仍 push(历史保真,刷新语义)", () => {
    const s = navGo({ back: [], fwd: [], current: B }, B);
    expect(s.back).toEqual([B]);
    expect(s.current).toEqual(B);
  });
});

describe("navBack / navForward", () => {
  it("栈序正确:back 弹顶入 fwd,fwd 弹顶入 back", () => {
    // 构造链:A → B → C → D,然后连续后退再连续前进。
    let s: NavState = { back: [], fwd: [], current: A };
    s = navGo(s, B);
    s = navGo(s, C);
    s = navGo(s, { kind: "dir", device: "dev-1", path: "C:\\docs\\src\\out" });
    const D = s.current;

    s = navBack(s) as NavState; // → C
    expect(s.current).toEqual(C);
    expect(s.fwd).toEqual([D]);
    s = navBack(s) as NavState; // → B
    expect(s.current).toEqual(B);
    expect(s.fwd).toEqual([C, D]);
    s = navBack(s) as NavState; // → A
    expect(s.current).toEqual(A);
    expect(s.back).toEqual([]);
    expect(s.fwd).toEqual([B, C, D]);
    expect(navBack(s)).toBe(null); // back 空 → null

    s = navForward(s) as NavState; // → B
    expect(s.current).toEqual(B);
    expect(s.back).toEqual([A]);
    expect(s.fwd).toEqual([C, D]);
    s = navForward(s) as NavState; // → C
    expect(s.current).toEqual(C);
    expect(s.fwd).toEqual([D]);
    s = navForward(s) as NavState; // → D
    expect(s.current).toEqual(D);
    expect(s.fwd).toEqual([]);
    expect(navForward(s)).toBe(null); // fwd 空 → null
  });

  it("后退/前进清空对侧后仍保持正确栈序", () => {
    const withFwd: NavState = { back: [A], fwd: [C], current: B };
    const after = navGo(withFwd, dir("dev-1", "C:\\other"));
    expect(after.fwd).toEqual([]); // navGo 清 fwd
    expect(after.back).toEqual([A, B]);
  });

  it("fav 位携入历史无损:navGo 保真,navBack 完整返回前位", () => {
    let s: NavState = { back: [], fwd: [], current: A };
    s = navGo(s, B);
    s = navGo(s, F1); // 进入快捷目录视图
    expect(s.current).toEqual(F1);
    expect(s.back).toEqual([A, B]);
    expect(s.fwd).toEqual([]);
    s = navBack(s) as NavState; // 从 fav 后退 → B
    expect(s.current).toEqual(B);
    expect(s.fwd).toEqual([F1]);
    s = navForward(s) as NavState; // 前进回 fav 位,id 不变
    expect(s.current).toEqual(F1);
  });

  it("favdir 位携入历史无损:混合视图 → 成员根 → 子目录全链保真(返工 §13.6)", () => {
    let s: NavState = { back: [], fwd: [], current: A };
    s = navGo(s, F1); // 快捷混合视图
    s = navGo(s, FD_ROOT); // 钻取成员根(rel="")
    s = navGo(s, FD_SUB); // 快捷命名空间内逐级
    expect(s.current).toEqual(FD_SUB);
    expect(s.back).toEqual([A, F1, FD_ROOT]);
    s = navBack(s) as NavState; // 上级 → 成员根
    expect(s.current).toEqual(FD_ROOT);
    s = navBack(s) as NavState; // 顶级 → 混合视图
    expect(s.current).toEqual(F1);
    s = navForward(s) as NavState; // 前进按原链回子目录位
    expect(s.current).toEqual(FD_ROOT);
    s = navForward(s) as NavState;
    expect(s.current).toEqual(FD_SUB);
    expect(s.fwd).toEqual([]);
  });
});

describe("导航纯函数性", () => {
  it("navGo / navBack / navForward 均不改入参状态", () => {
    const h: NavState = { back: [A], fwd: [C], current: B };
    const snapshot = { back: [...h.back], fwd: [...h.fwd], current: h.current };
    navGo(h, A);
    navBack(h);
    navForward(h);
    expect(h).toEqual(snapshot);
  });
});
