import { describe, expect, it } from "vitest";

import type { FsEntry } from "./browse";
import {
  DEVICE_COLOR_HEX,
  EMPTY_FAVS,
  FAVORITES_KV_KEY,
  type FavMember,
  type FavoritesDoc,
  type Favorite,
  addMembers,
  createFavorite,
  deviceColorMap,
  favAddTargets,
  memberKey,
  newFavId,
  parseFavorites,
  removeFavorite,
  resolveFavDropTarget,
  removeMember,
  renameFavorite,
} from "./favorites";

/// 便捷构造成员。
function m(device: string, path: string): FavMember {
  return { device, path };
}

/// 便捷构造常用条目(created_at 默认 1000)。
function fav(id: string, name: string, members: FavMember[] = [], created_at = 1000): Favorite {
  return { id, name, created_at, members };
}

/// 便捷构造整单文档(updated_at 默认 0)。
function doc(favorites: Favorite[], updated_at = 0): FavoritesDoc {
  return { v: 1, updated_at, favorites };
}

describe("FAVORITES_KV_KEY 与 EMPTY_FAVS", () => {
  it("kv 键为 favorites(ASCII 定死)", () => {
    expect(FAVORITES_KV_KEY).toBe("favorites");
  });

  it("EMPTY_FAVS 为空 v1 文档", () => {
    expect(EMPTY_FAVS).toEqual({ v: 1, updated_at: 0, favorites: [] });
  });
});

describe("parseFavorites", () => {
  it("null / 空串 → null(无键合法态)", () => {
    expect(parseFavorites(null)).toBe(null);
    expect(parseFavorites("")).toBe(null);
  });

  it("合法 v1 往返解析,readOnly=false", () => {
    const d = doc([fav("f1", "工作资料", [m("d1", "C:\\工作"), m("d2", "/home/u/docs")])], 1730000000000);
    const p = parseFavorites(JSON.stringify(d));
    expect(p).toEqual({ doc: d, readOnly: false });
  });

  it("字段缺失容忍:updated_at→0、favorites→[]、条目 created_at→0、members→[]", () => {
    const p = parseFavorites(JSON.stringify({ v: 1 }));
    expect(p).toEqual({ doc: { v: 1, updated_at: 0, favorites: [] }, readOnly: false });

    const p2 = parseFavorites(JSON.stringify({ v: 1, favorites: [{ id: "f1", name: "N" }] }));
    expect(p2).toEqual({ doc: doc([fav("f1", "N", [], 0)]), readOnly: false });
  });

  it("v>1 → readOnly=true 且已知字段尽力解析", () => {
    const raw = JSON.stringify({ ...doc([fav("f1", "A", [m("d1", "C:\\x")], 9)], 55), v: 2 });
    const p = parseFavorites(raw);
    expect(p).not.toBe(null);
    expect(p!.readOnly).toBe(true);
    expect(p!.doc).toEqual(doc([fav("f1", "A", [m("d1", "C:\\x")], 9)], 55));
  });

  it("解码失败(非 JSON)→ readOnly=true + 空文档", () => {
    expect(parseFavorites("not json")).toEqual({ doc: EMPTY_FAVS, readOnly: true });
  });

  it("JSON 合法但非对象 / 无 v 可读 → readOnly=true + 空文档", () => {
    expect(parseFavorites("42")).toEqual({ doc: EMPTY_FAVS, readOnly: true });
    expect(parseFavorites("[]")).toEqual({ doc: EMPTY_FAVS, readOnly: true });
    expect(parseFavorites("{}")).toEqual({ doc: EMPTY_FAVS, readOnly: true });
    expect(parseFavorites(JSON.stringify({ updated_at: 5, favorites: [] }))).toEqual({
      doc: EMPTY_FAVS,
      readOnly: true,
    });
  });
});

describe("newFavId", () => {
  it("16 位小写十六进制,多次生成互不相同", () => {
    const ids = new Set(Array.from({ length: 64 }, () => newFavId()));
    expect(ids.size).toBe(64);
    for (const id of ids) expect(id).toMatch(/^[0-9a-f]{16}$/);
  });
});

describe("createFavorite", () => {
  it("尾部追加(创建序),created_at/updated_at 取 now,空成员合法", () => {
    const d2 = createFavorite(createFavorite(doc([]), "一", 111), "二", 222);
    expect(d2.favorites.map((f) => f.name)).toEqual(["一", "二"]);
    expect(d2.favorites[0]).toMatchObject({ created_at: 111, members: [] });
    expect(d2.favorites[1]).toMatchObject({ created_at: 222, members: [] });
    expect(d2.updated_at).toBe(222);
    expect(d2.v).toBe(1);
  });

  it("id 唯一", () => {
    const d = createFavorite(createFavorite(doc([]), "a", 1), "b", 2);
    expect(d.favorites[0].id).not.toBe(d.favorites[1].id);
  });

  it("纯函数:不改入参", () => {
    const d0 = doc([fav("f1", "A")]);
    const snapshot = JSON.stringify(d0);
    createFavorite(d0, "B", 2);
    expect(JSON.stringify(d0)).toBe(snapshot);
  });
});

describe("renameFavorite", () => {
  it("只改目标名,其余条目不动", () => {
    const d = doc([fav("f1", "旧名"), fav("f2", "乙")]);
    const out = renameFavorite(d, "f1", "新名");
    expect(out.favorites[0]).toMatchObject({ id: "f1", name: "新名" });
    expect(out.favorites[1]).toEqual(fav("f2", "乙"));
  });

  it("id 不存在 → 等值返回", () => {
    const d = doc([fav("f1", "A")]);
    expect(renameFavorite(d, "nope", "X")).toEqual(d);
  });
});

describe("removeFavorite", () => {
  it("只删目标,其余条目不动", () => {
    const d = doc([fav("f1", "A"), fav("f2", "B"), fav("f3", "C")]);
    const out = removeFavorite(d, "f2");
    expect(out.favorites.map((f) => f.id)).toEqual(["f1", "f3"]);
    expect(out.v).toBe(1);
  });

  it("id 不存在 → 等值返回", () => {
    const d = doc([fav("f1", "A")]);
    expect(removeFavorite(d, "nope")).toEqual(d);
  });
});

describe("addMembers", () => {
  it("同 (device,path) 在该常用内去重,added 计数准确,成员保添加序", () => {
    const d = doc([fav("f1", "A", [m("d1", "C:\\a")])]);
    const r = addMembers(d, "f1", [m("d1", "C:\\a"), m("d1", "C:\\b"), m("d2", "C:\\a"), m("d1", "C:\\b")]);
    expect(r.added).toBe(2);
    expect(r.doc.favorites[0].members).toEqual([m("d1", "C:\\a"), m("d1", "C:\\b"), m("d2", "C:\\a")]);
  });

  it("重复添加已有成员 → added 0(静默去重)", () => {
    const d = doc([fav("f1", "A", [m("d1", "C:\\a")])]);
    const r = addMembers(d, "f1", [m("d1", "C:\\a")]);
    expect(r.added).toBe(0);
    expect(r.doc.favorites[0].members).toEqual([m("d1", "C:\\a")]);
  });

  it("跨常用重复允许", () => {
    const d = doc([fav("f1", "A"), fav("f2", "B")]);
    const r1 = addMembers(d, "f1", [m("d1", "C:\\a")]);
    const r2 = addMembers(r1.doc, "f2", [m("d1", "C:\\a")]);
    expect(r1.added).toBe(1);
    expect(r2.added).toBe(1);
    expect(r2.doc.favorites[0].members).toEqual([m("d1", "C:\\a")]);
    expect(r2.doc.favorites[1].members).toEqual([m("d1", "C:\\a")]);
  });

  it("常用不存在 → added 0,文档不变", () => {
    const d = doc([fav("f1", "A")]);
    const r = addMembers(d, "nope", [m("d1", "C:\\a")]);
    expect(r.added).toBe(0);
    expect(r.doc).toEqual(d);
  });

  it("纯函数:不改入参", () => {
    const d = doc([fav("f1", "A")]);
    const snapshot = JSON.stringify(d);
    addMembers(d, "f1", [m("d1", "C:\\a")]);
    expect(JSON.stringify(d)).toBe(snapshot);
  });
});

describe("removeMember", () => {
  it("只删目标常用内的匹配成员,其他成员与其他常用不动", () => {
    const d = doc([fav("f1", "A", [m("d1", "C:\\a"), m("d1", "C:\\b")]), fav("f2", "B", [m("d1", "C:\\a")])]);
    const out = removeMember(d, "f1", m("d1", "C:\\a"));
    expect(out.favorites[0].members).toEqual([m("d1", "C:\\b")]);
    expect(out.favorites[1].members).toEqual([m("d1", "C:\\a")]);
  });

  it("成员或常用不存在 → 等值返回", () => {
    const d = doc([fav("f1", "A", [m("d1", "C:\\a")])]);
    expect(removeMember(d, "f1", m("d9", "C:\\x"))).toEqual(d);
    expect(removeMember(d, "nope", m("d1", "C:\\a"))).toEqual(d);
  });
});

describe("memberKey", () => {
  it("device + NUL(\\u0000) + path 拼接", () => {
    expect(memberKey(m("d1", "C:\\a"))).toBe("d1\u0000C:\\a");
  });
});

describe("deviceColorMap", () => {
  it("六色板与 spec §4 逐字对齐", () => {
    expect(DEVICE_COLOR_HEX).toEqual(["#14B8A6", "#06B6D4", "#8B5CF6", "#D946EF", "#EC4899", "#64748B"]);
  });

  it("3 台按 device_id 字典序位次取色(乱序输入同结果)", () => {
    const map = deviceColorMap([{ device_id: "c" }, { device_id: "a" }, { device_id: "b" }]);
    expect(map.get("a")).toBe(DEVICE_COLOR_HEX[0]);
    expect(map.get("b")).toBe(DEVICE_COLOR_HEX[1]);
    expect(map.get("c")).toBe(DEVICE_COLOR_HEX[2]);
  });

  it(">6 台回绕:第 7 台 = 第 1 色", () => {
    const ids = ["d1", "d2", "d3", "d4", "d5", "d6", "d7"];
    const map = deviceColorMap(ids.map((device_id) => ({ device_id })));
    expect(map.get("d1")).toBe(DEVICE_COLOR_HEX[0]);
    expect(map.get("d7")).toBe(DEVICE_COLOR_HEX[0]);
    expect(map.get("d7")).toBe(map.get("d1"));
    expect(map.get("d4")).toBe(DEVICE_COLOR_HEX[3]);
  });

  it("纯函数:不改入参数组顺序", () => {
    const input = [{ device_id: "b" }, { device_id: "a" }];
    deviceColorMap(input);
    expect(input.map((d) => d.device_id)).toEqual(["b", "a"]);
  });
});

describe("favAddTargets(V3-3 Task 11:目录位「添加到常用目录」候选与门)", () => {
  /// 便捷构造条目。
  function e(path: string, isDir: boolean): FsEntry {
    return { name: path, path, size: 1, mtime_ms: 0, is_dir: isDir };
  }
  const dirA = e("C:\a", true);
  const dirB = e("C:\b", true);
  const fileC = e("C:\c.txt", false);

  it("右键目录 + 选择空 → 单条候选该行", () => {
    expect(favAddTargets(dirA, [dirA], new Set())).toEqual(["C:\a"]);
  });

  it("右键目录且在选择内(单选)→ 单条候选该行", () => {
    expect(favAddTargets(dirA, [dirA], new Set(["C:\a"]))).toEqual(["C:\a"]);
  });

  it("右键目录且多选全目录(含该行)→ 全部选中目录", () => {
    expect(favAddTargets(dirA, [dirA, dirB, fileC], new Set(["C:\a", "C:\b"]))).toEqual(["C:\a", "C:\b"]);
  });

  it("右键目录但选择混有文件(含该行)→ 只取右键行(全目录门不满足)", () => {
    expect(favAddTargets(dirA, [dirA, fileC], new Set(["C:\a", "C:\c.txt"]))).toEqual(["C:\a"]);
  });

  it("右键文件 → null(菜单项不呈现)", () => {
    expect(favAddTargets(fileC, [fileC], new Set(["C:\c.txt"]))).toBeNull();
  });

  it("右键目录但选择不含该行 → null(防御:行右键必先单选)", () => {
    expect(favAddTargets(dirA, [dirA, dirB], new Set(["C:\b"]))).toBeNull();
  });

  it("空白处 + 多选全目录 → 全部选中目录", () => {
    expect(favAddTargets(null, [dirA, dirB], new Set(["C:\a", "C:\b"]))).toEqual(["C:\a", "C:\b"]);
  });

  it("空白处 + 选择混有文件 / 空 → null", () => {
    expect(favAddTargets(null, [dirA, fileC], new Set(["C:\a", "C:\c.txt"]))).toBeNull();
    expect(favAddTargets(null, [dirA], new Set())).toBeNull();
  });

  it("纯函数:不改入参", () => {
    const rows = [dirA, dirB];
    const sel = new Set(["C:\a", "C:\b"]);
    favAddTargets(dirA, rows, sel);
    expect(rows.map((r) => r.path)).toEqual(["C:\a", "C:\b"]);
    expect([...sel]).toEqual(["C:\a", "C:\b"]);
  });
});

describe("resolveFavDropTarget(V3-3 Task 12:常用拖放/粘贴落点解析)", () => {
  it("单成员 → 直取该成员(等效 device\\u0000path 落点)", () => {
    const fav: Favorite = { id: "f1", name: "n", created_at: 0, members: [{ device: "d1", path: "C:\\a" }] };
    expect(resolveFavDropTarget(fav)).toEqual({ kind: "member", member: { device: "d1", path: "C:\\a" } });
  });

  it("多成员 → picker(开成员选择层)", () => {
    const fav: Favorite = {
      id: "f2",
      name: "n",
      created_at: 0,
      members: [
        { device: "d1", path: "C:\\a" },
        { device: "d2", path: "/b" },
      ],
    };
    expect(resolveFavDropTarget(fav)).toEqual({ kind: "picker" });
  });

  it("空常用(0 成员)→ null(无落点,调用方忽略)", () => {
    const fav: Favorite = { id: "f3", name: "n", created_at: 0, members: [] };
    expect(resolveFavDropTarget(fav)).toBeNull();
  });

  it("纯函数:不改入参", () => {
    const fav: Favorite = { id: "f4", name: "n", created_at: 0, members: [{ device: "d1", path: "C:\\a" }] };
    resolveFavDropTarget(fav);
    expect(fav.members).toEqual([{ device: "d1", path: "C:\\a" }]);
  });
});
