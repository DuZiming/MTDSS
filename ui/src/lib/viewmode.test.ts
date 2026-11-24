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
  createViewModeMemory,
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

describe("viewmode 键面(field r24 #6 用户裁定:视图全局有效,单一全局键)", () => {
  it("dir/favdir/fav 位:同返 viewmode:global(与位置无关)", () => {
    const loc: Loc = { kind: "dir", device: "dev-1", path: "C:\\相册" };
    expect(viewModeKey(loc)).toBe("viewmode:global");
    const favdir: Loc = { kind: "favdir", favId: "f1", device: "dev-2", base: "C:\\a", rel: "b\\c" };
    expect(viewModeKey(favdir)).toBe("viewmode:global");
    const fav: Loc = { kind: "fav", id: "ab12cd34" };
    expect(viewModeKey(fav)).toBe("viewmode:global");
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

describe("createViewModeMemory(fix round 1 #2:显式切换赢在飞装载)", () => {
  /// 微任务清排(pending kvGet 的受控 resolve 全 Promise 面)。
  const flush = async (): Promise<void> => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  };

  it("切换发生在装载在途:迟到应答弃,用户选择保态,回写不被覆盖", async () => {
    let resolveGet!: (v: string | null) => void;
    kvGetMock.mockReturnValue(
      new Promise<string | null>((res) => {
        resolveGet = res;
      }),
    );
    kvPutMock.mockResolvedValue(undefined);
    const mem = createViewModeMemory();
    const load = mem.load("viewmode:d:p"); // 导航触发装载,在途
    mem.change("tiles", "viewmode:d:p"); // 用户显式切 tiles(翻代+回写)
    expect(mem.get()).toBe("tiles");
    expect(kvPutMock).toHaveBeenCalledWith("viewmode:d:p", "tiles");
    resolveGet("details"); // 迟到应答(kv 记忆面为旧值/缺省)
    await load;
    await flush();
    expect(mem.get()).toBe("tiles"); // 用户选择赢,不被覆回
    expect(kvPutMock).toHaveBeenCalledTimes(1); // 回写不被后续写覆盖
  });

  it("切换发生在装载在途+kvGet 拒(离线缺省面):迟到缺省同样弃", async () => {
    let rejectGet!: (e: unknown) => void;
    kvGetMock.mockReturnValue(
      new Promise<string | null>((_, rej) => {
        rejectGet = rej;
      }),
    );
    kvPutMock.mockResolvedValue(undefined);
    const mem = createViewModeMemory();
    const load = mem.load("viewmode:d:p");
    mem.change("list", "viewmode:d:p");
    rejectGet(new Error("code 3: offline"));
    await load;
    await flush();
    expect(mem.get()).toBe("list");
  });

  it("新装载翻代旧装载(换目录):仅最新键应答落态", async () => {
    const pendings: Array<(v: string | null) => void> = [];
    kvGetMock.mockImplementation(
      () => new Promise<string | null>((res) => pendings.push(res)),
    );
    const mem = createViewModeMemory();
    const loadA = mem.load("viewmode:d:a");
    const loadB = mem.load("viewmode:d:b"); // 换目录,翻代 A
    pendings[1]?.("list");
    await loadB;
    expect(mem.get()).toBe("list");
    pendings[0]?.("tiles"); // A 迟到应答
    await loadA;
    await flush();
    expect(mem.get()).toBe("list");
  });

  it("正常装载落态并通知订阅;change(null 键)落态不写 kv", async () => {
    kvGetMock.mockResolvedValue("tiles");
    kvPutMock.mockClear();
    const mem = createViewModeMemory();
    const seen: string[] = [];
    mem.subscribe(() => seen.push(mem.get()));
    await mem.load("viewmode:d:p");
    expect(mem.get()).toBe("tiles");
    expect(seen).toEqual(["tiles"]);
    mem.change("details", null); // 着陆位:翻代不写 kv
    await flush();
    expect(mem.get()).toBe("details");
    expect(kvPutMock).not.toHaveBeenCalled();
    expect(seen).toEqual(["tiles", "details"]);
  });
});
