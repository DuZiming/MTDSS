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
  favdirAt,
  memberKey,
  memberOfPath,
  newFavId,
  parseFavorites,
  removeFavorite,
  resolveFavDropTarget,
  removeMember,
  renameFavorite,
} from "./favorites";
import { joinRel } from "./nav";

/// 便捷构造成员。
function m(device: string, path: string): FavMember {
  return { device, path };
}

/// 便捷构造快捷条目(created_at 默认 1000)。
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
  it("同 (device,path) 在该快捷内去重,added 计数准确,成员保添加序", () => {
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

  it("跨快捷重复允许", () => {
    const d = doc([fav("f1", "A"), fav("f2", "B")]);
    const r1 = addMembers(d, "f1", [m("d1", "C:\\a")]);
    const r2 = addMembers(r1.doc, "f2", [m("d1", "C:\\a")]);
    expect(r1.added).toBe(1);
    expect(r2.added).toBe(1);
    expect(r2.doc.favorites[0].members).toEqual([m("d1", "C:\\a")]);
    expect(r2.doc.favorites[1].members).toEqual([m("d1", "C:\\a")]);
  });

  it("快捷不存在 → added 0,文档不变", () => {
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
  it("只删目标快捷内的匹配成员,其他成员与其他快捷不动", () => {
    const d = doc([fav("f1", "A", [m("d1", "C:\\a"), m("d1", "C:\\b")]), fav("f2", "B", [m("d1", "C:\\a")])]);
    const out = removeMember(d, "f1", m("d1", "C:\\a"));
    expect(out.favorites[0].members).toEqual([m("d1", "C:\\b")]);
    expect(out.favorites[1].members).toEqual([m("d1", "C:\\a")]);
  });

  it("成员或快捷不存在 → 等值返回", () => {
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

describe("deviceColorMap(§13.4 返工钦定:64 色板 + 加入顺序位次)", () => {
  /// 便捷构构设备(created_at 可缺,缺省=旧 daemon 形态)。
  function dev(device_id: string, created_at?: number): { device_id: string; created_at?: number } {
    return created_at === undefined ? { device_id } : { device_id, created_at };
  }

  it("64 色板与 spec §13.4 json 块逐字节对齐(用户逐格钦定 verbatim)", () => {
    expect(DEVICE_COLOR_HEX).toHaveLength(64);
    expect(DEVICE_COLOR_HEX).toEqual([
      "#158cd5", "#d52c15", "#2fd515", "#d58515", "#1548d5", "#d5158f", "#15cfd5", "#7f15d5",
      "#1679b6", "#b62916", "#2cb616", "#b67316", "#1641b6", "#b6167b", "#16b0b6", "#6e16b6",
      "#1e9deb", "#eb361e", "#39eb1e", "#eb961e", "#1e55eb", "#eb1ea0", "#1ee5eb", "#8f1eeb",
      "#1a6593", "#93281a", "#2a931a", "#93611a", "#1a3a93", "#931a67", "#1a8f93", "#5d1a93",
      "#3faef3", "#f3543f", "#57f33f", "#f3a83f", "#3f6ff3", "#f33fb1", "#3fedf3", "#a23ff3",
      "#1f516f", "#6f291f", "#2a6f1f", "#6f4e1f", "#1f356f", "#6f1f52", "#1f6d6f", "#4b1f6f",
      "#1f3e51", "#51251f", "#26511f", "#513c1f", "#1f2d51", "#511f3f", "#1f4f51", "#3b1f51",
      "#1b2c37", "#371e1b", "#1f371b", "#372b1b", "#1b2237", "#371b2c", "#1b3637", "#2a1b37",
    ]);
  });

  it("首/末与采样位次钉死(行主序 8 色系×8 明暗档)", () => {
    expect(DEVICE_COLOR_HEX[0]).toBe("#158cd5");
    expect(DEVICE_COLOR_HEX[7]).toBe("#7f15d5");
    expect(DEVICE_COLOR_HEX[8]).toBe("#1679b6");
    expect(DEVICE_COLOR_HEX[32]).toBe("#3faef3");
    expect(DEVICE_COLOR_HEX[63]).toBe("#2a1b37");
    expect(new Set(DEVICE_COLOR_HEX).size).toBe(64);
  });

  it("按 created_at 升序取位(乱序输入同结果)", () => {
    const map = deviceColorMap([dev("c", 30), dev("a", 10), dev("b", 20)]);
    expect(map.get("a")).toBe(DEVICE_COLOR_HEX[0]);
    expect(map.get("b")).toBe(DEVICE_COLOR_HEX[1]);
    expect(map.get("c")).toBe(DEVICE_COLOR_HEX[2]);
  });

  it("created_at 并列 → 按 device_id 定序", () => {
    const map = deviceColorMap([dev("z", 5), dev("a", 5), dev("m", 5)]);
    expect(map.get("a")).toBe(DEVICE_COLOR_HEX[0]);
    expect(map.get("m")).toBe(DEVICE_COLOR_HEX[1]);
    expect(map.get("z")).toBe(DEVICE_COLOR_HEX[2]);
  });

  it("缺 created_at(旧 daemon)排尾,彼此按 device_id", () => {
    const map = deviceColorMap([dev("z"), dev("b", 10), dev("a"), dev("a2", 20)]);
    expect(map.get("b")).toBe(DEVICE_COLOR_HEX[0]);
    expect(map.get("a2")).toBe(DEVICE_COLOR_HEX[1]);
    expect(map.get("a")).toBe(DEVICE_COLOR_HEX[2]);
    expect(map.get("z")).toBe(DEVICE_COLOR_HEX[3]);
  });

  it(">64 台回绕:第 65 台 = 第 1 色", () => {
    const devices = Array.from({ length: 65 }, (_, i) => dev(`d${i}`, i));
    const map = deviceColorMap(devices);
    expect(map.get("d0")).toBe(DEVICE_COLOR_HEX[0]);
    expect(map.get("d64")).toBe(DEVICE_COLOR_HEX[0]);
    expect(map.get("d64")).toBe(map.get("d0"));
    expect(map.get("d32")).toBe(DEVICE_COLOR_HEX[32]);
  });

  it("纯函数:不改入参数组顺序", () => {
    const input = [dev("b", 2), dev("a", 1)];
    deviceColorMap(input);
    expect(input.map((d) => d.device_id)).toEqual(["b", "a"]);
  });
});

describe("favAddTargets(V3-3 Task 11:目录位「添加到快捷目录」候选与门)", () => {
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

describe("resolveFavDropTarget(V3-3 Task 12:快捷拖放/粘贴落点解析)", () => {
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

  it("空快捷(0 成员)→ null(无落点,调用方忽略)", () => {
    const fav: Favorite = { id: "f3", name: "n", created_at: 0, members: [] };
    expect(resolveFavDropTarget(fav)).toBeNull();
  });

  it("纯函数:不改入参", () => {
    const fav: Favorite = { id: "f4", name: "n", created_at: 0, members: [{ device: "d1", path: "C:\\a" }] };
    resolveFavDropTarget(fav);
    expect(fav.members).toEqual([{ device: "d1", path: "C:\\a" }]);
  });
});

describe("memberOfPath(V3-3 返工 §13.6:favdir 钻取 base 判定)", () => {
  const fav: Favorite = {
    id: "f1",
    name: "n",
    created_at: 0,
    members: [
      { device: "d1", path: "C:\\docs" },
      { device: "d1", path: "C:\\docs\\src" }, // 同设备嵌套成员
      { device: "d2", path: "/srv" },
    ],
  };

  it("唯一含该点的成员直取(跨设备过滤)", () => {
    expect(memberOfPath(fav, "d2", "/srv/data")).toEqual({ device: "d2", path: "/srv" });
    expect(memberOfPath(fav, "d1", "C:\\docs\\readme")).toEqual({ device: "d1", path: "C:\\docs" });
  });

  it("同设备嵌套成员取最深(最长前缀)", () => {
    expect(memberOfPath(fav, "d1", "C:\\docs\\src\\out")).toEqual({ device: "d1", path: "C:\\docs\\src" });
  });

  it("child 即成员本身也算包含", () => {
    expect(memberOfPath(fav, "d1", "C:\\docs\\src")).toEqual({ device: "d1", path: "C:\\docs\\src" });
  });

  it("无成员包含 → null(调用方防御回退)", () => {
    expect(memberOfPath(fav, "d9", "C:\\docs")).toBeNull();
    expect(memberOfPath(fav, "d1", "C:\\docsx")).toBeNull(); // 前缀非段边界
  });

  it("纯函数:不改入参", () => {
    const snap = JSON.stringify(fav);
    memberOfPath(fav, "d1", "C:\\docs\\src\\out");
    expect(JSON.stringify(fav)).toBe(snap);
  });
});

describe("favdirAt(V3-3 返工 §13.6 R6 修复轮:任意深度节点 → 规范 base/rel)", () => {
  const fav: Favorite = {
    id: "f1",
    name: "n",
    created_at: 0,
    members: [
      { device: "d1", path: "C:\\docs" },
      { device: "d1", path: "C:\\docs\\src" }, // 同设备嵌套成员
      { device: "d2", path: "/srv" },
    ],
  };

  it("成员根本身(侧栏成员节点)→ base=成员路径,rel=\"\"", () => {
    expect(favdirAt(fav, "d1", "C:\\docs")).toEqual({ base: "C:\\docs", rel: "" });
    expect(favdirAt(fav, "d2", "/srv")).toEqual({ base: "/srv", rel: "" });
  });

  it("成员子目录(侧栏懒加载嵌套节点)→ base=成员路径,rel=相对段(非 base=点击路径)", () => {
    expect(favdirAt(fav, "d1", "C:\\docs\\src\\out")).toEqual({
      base: "C:\\docs\\src",
      rel: "out",
    });
    expect(favdirAt(fav, "d1", "C:\\docs\\readme")).toEqual({ base: "C:\\docs", rel: "readme" });
  });

  it("同设备嵌套成员取最深为 base", () => {
    expect(favdirAt(fav, "d1", "C:\\docs\\src")).toEqual({ base: "C:\\docs\\src", rel: "" });
  });

  it("无成员包含 → null(调用方防御回退)", () => {
    expect(favdirAt(fav, "d9", "C:\\docs")).toBeNull();
    expect(favdirAt(fav, "d1", "C:\\docsx")).toBeNull();
  });

  it("联用回环:joinRel(base, rel) 复原点击绝对路径", () => {
    const at = favdirAt(fav, "d1", "C:\\docs\\src\\out");
    expect(at).not.toBeNull();
    expect(joinRel((at as { base: string; rel: string }).base, (at as { base: string; rel: string }).rel)).toBe(
      "C:\\docs\\src\\out",
    );
  });
});
