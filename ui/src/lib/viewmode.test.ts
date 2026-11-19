/// 视图模式记忆(V4-1 T19)单测:每目录 kv 键面 + 解析 + 尽力而为读写。
/// kv 依赖走 vi.mock(stub,直驱纯函数——favstore.test 同手法)。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const kvGetMock = vi.fn();
const kvPutMock = vi.fn();

vi.mock("./daemon", () => ({
  kvGet: (...args: unknown[]) => kvGetMock(...args),
  kvPut: (...args: unknown[]) => kvPutMock(...args),
}));

import type { Loc } from "./nav";
import {
  VIEWMODE_DEFAULT,
  loadViewMode,
  parseViewMode,
  saveViewMode,
  viewModeKey,
} from "./viewmode";

beforeEach(() => {
  kvGetMock.mockReset();
  kvPutMock.mockReset();
});
afterEach(() => {
  vi.restoreAllMocks();
});

describe("viewmode 键面(brief 钉死 viewmode:{device}:{dir})", () => {
  it("dir 位:device+path 原串直拼(收件箱即普通 dir 位,天然覆盖)", () => {
    const loc: Loc = { kind: "dir", device: "dev-1", path: "C:\\相册" };
    expect(viewModeKey(loc)).toBe("viewmode:dev-1:C:\\相册");
  });

  it("favdir 位:成员原生路径(joinRel(base,rel) 与浏览管线同源)——同一物理目录与 dir 位同键记忆", () => {
    const loc: Loc = { kind: "favdir", favId: "f1", device: "dev-2", base: "C:\\a", rel: "b\\c" };
    expect(viewModeKey(loc)).toBe("viewmode:dev-2:C:\\a\\b\\c");
    const root: Loc = { kind: "favdir", favId: "f1", device: "dev-2", base: "C:\\a", rel: "" };
    expect(viewModeKey(root)).toBe("viewmode:dev-2:C:\\a");
  });

  it("fav 位(混合视图):favorites 复合键先例——实体以稳定 id 访名", () => {
    const loc: Loc = { kind: "fav", id: "ab12cd34" };
    expect(viewModeKey(loc)).toBe("viewmode:fav:ab12cd34");
  });

  it("devices/favs 着陆位:非目录视图,null=不记忆", () => {
    expect(viewModeKey({ kind: "devices" })).toBeNull();
    expect(viewModeKey({ kind: "favs" })).toBeNull();
  });
});

describe("parseViewMode(白名单解析)", () => {
  it("三态字面量透传;null/未知值/空串 → 缺省 details", () => {
    expect(parseViewMode("tiles")).toBe("tiles");
    expect(parseViewMode("list")).toBe("list");
    expect(parseViewMode("details")).toBe("details");
    expect(parseViewMode(null)).toBe(VIEWMODE_DEFAULT);
    expect(parseViewMode("")).toBe(VIEWMODE_DEFAULT);
    expect(parseViewMode("gallery")).toBe(VIEWMODE_DEFAULT);
  });
});

describe("loadViewMode / saveViewMode(尽力而为,浏览不因记忆失败)", () => {
  it("kvGet 有值 → 记忆恢复(切换记忆恢复主面)", async () => {
    kvGetMock.mockResolvedValue("tiles");
    await expect(loadViewMode("viewmode:d:p")).resolves.toBe("tiles");
    expect(kvGetMock).toHaveBeenCalledWith("viewmode:d:p");
  });

  it("kvGet null(键不存在)/ reject(离线等)→ 缺省,不抛", async () => {
    kvGetMock.mockResolvedValue(null);
    await expect(loadViewMode("k")).resolves.toBe(VIEWMODE_DEFAULT);
    kvGetMock.mockRejectedValue(new Error("code 3: offline"));
    await expect(loadViewMode("k")).resolves.toBe(VIEWMODE_DEFAULT);
  });

  it("saveViewMode 走 kvPut(key, 模式串);失败静默不抛", async () => {
    kvPutMock.mockResolvedValue(undefined);
    await saveViewMode("viewmode:d:p", "list");
    expect(kvPutMock).toHaveBeenCalledWith("viewmode:d:p", "list");
    kvPutMock.mockRejectedValue(new Error("code 3: offline"));
    await expect(saveViewMode("viewmode:d:p", "tiles")).resolves.toBeUndefined();
  });
});
