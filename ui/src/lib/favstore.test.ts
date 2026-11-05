/// useFavorites store 行为测试(V3-3 Task 4,六场景钉死):
/// ①登录翻转→kvGet 首拉/登出复位 EMPTY 并解订;②kvGet null→EMPTY+ready;
/// ③v:2 前向守卫→readOnly、createFav false、kvPut 零调用;
/// ④createFav 前置 kv.get 取最新(两次不同返回,put=并发条目+新条目);
/// ⑤kvPut reject→状态保旧值、不自动重试;⑥data_changed key=favorites→
/// refresh,key 不符→不刷。另补 addMembers 计数/零新增不回写与其余编辑 op
/// 同面复用(put 成功才落状态、store 统一盖 updated_at)。
///
/// 仓库测试设施为 node 环境 vitest(无 jsdom/@testing-library,SSR 渲染
/// 不跑 effects),hook 逻辑收口在 createFavoritesCore(React 无关引擎,
/// useFavorites 为其 useState/useEffect 薄接线);测试直驱引擎,行为面
/// 与 hook 等价。
import { beforeEach, describe, expect, it, vi } from "vitest";

const kvGetMock = vi.fn();
const kvPutMock = vi.fn();
const unlistenMock = vi.fn();
/// listenDataChanged 捕获的处理器(测试据此模拟 daemon 推送)。
let dataChangedCb: ((key: string) => void) | null = null;

vi.mock("./daemon", () => ({
  kvGet: (...args: unknown[]) => kvGetMock(...args),
  kvPut: (...args: unknown[]) => kvPutMock(...args),
  listenDataChanged: (cb: (key: string) => void) => {
    dataChangedCb = cb;
    return Promise.resolve(() => {
      dataChangedCb = null;
      unlistenMock();
    });
  },
}));

import { EMPTY_FAVS, FAVORITES_KV_KEY, parseFavorites } from "./favorites";
import { createFavoritesCore } from "./favstore";

/// 排干微任务:mock 全部同步 resolve,一个宏任务tick后所有链已收敛。
const flush = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

/// 构造 kv 存值串(updated_at 固定历史值,便于断言 store 有盖新戳)。
type SeedFav = { id: string; name: string; members?: { device: string; path: string }[] };
const seedDoc = (favs: SeedFav[], updatedAt = 1_700_000_000_000): string =>
  JSON.stringify({
    v: 1,
    updated_at: updatedAt,
    favorites: favs.map((f) => ({ ...f, created_at: updatedAt, members: f.members ?? [] })),
  });

beforeEach(() => {
  kvGetMock.mockReset();
  kvPutMock.mockReset();
  unlistenMock.mockReset();
  dataChangedCb = null;
});

describe("useFavorites(V3-3 Task 4 六场景)", () => {
  it("①登录 true→kvGet 首拉;登出→复位 EMPTY 并解订", async () => {
    kvGetMock.mockResolvedValue(null);
    const core = createFavoritesCore();

    core.setLoggedIn(true);
    await flush();
    expect(kvGetMock).toHaveBeenCalledTimes(1);
    expect(kvGetMock).toHaveBeenCalledWith(FAVORITES_KV_KEY);
    expect(core.state.ready).toBe(true);

    core.setLoggedIn(false);
    await flush();
    expect(unlistenMock).toHaveBeenCalledTimes(1);
    expect(core.state).toEqual({ doc: EMPTY_FAVS, readOnly: false, ready: false, busy: false });
  });

  it("②kvGet 返回 null(无键)→EMPTY 文档 + ready,可编辑", async () => {
    kvGetMock.mockResolvedValue(null);
    const core = createFavoritesCore();
    core.setLoggedIn(true);
    await flush();
    expect(core.state.doc).toEqual(EMPTY_FAVS);
    expect(core.state.readOnly).toBe(false);
    expect(core.state.ready).toBe(true);
  });

  it("③v:2→readOnly(已知字段尽力解析);createFav false 且 kvPut 零调用", async () => {
    kvGetMock.mockResolvedValue(
      JSON.stringify({ v: 2, updated_at: 42, favorites: [{ id: "new-v2", name: "新v2条目", created_at: 42, members: [] }] }),
    );
    const core = createFavoritesCore();
    core.setLoggedIn(true);
    await flush();
    expect(core.state.readOnly).toBe(true);
    expect(core.state.doc.favorites.map((f) => f.name)).toEqual(["新v2条目"]);

    const ok = await core.createFav("本地新建");
    expect(ok).toBe(false);
    expect(kvPutMock).not.toHaveBeenCalled();
    expect(core.state.busy).toBe(false);
  });

  it("④createFav 前置取最新:两次不同 kvGet,put=并发条目+新条目", async () => {
    const t0 = Date.now();
    kvGetMock
      .mockResolvedValueOnce(seedDoc([{ id: "seed-1", name: "已有条目" }]))
      .mockResolvedValueOnce(
        seedDoc([
          { id: "seed-1", name: "已有条目" },
          { id: "seed-2", name: "他端并发新增" },
        ]),
      );
    const core = createFavoritesCore();
    core.setLoggedIn(true);
    await flush();
    expect(core.state.doc.favorites).toHaveLength(1);

    const ok = await core.createFav("本地新建");
    expect(ok).toBe(true);
    expect(kvGetMock).toHaveBeenCalledTimes(2); // 编辑前置必再取一次
    expect(kvPutMock).toHaveBeenCalledTimes(1);
    const [putKey, putValue] = kvPutMock.mock.calls[0] as [string, string];
    expect(putKey).toBe(FAVORITES_KV_KEY);
    const put = parseFavorites(putValue);
    expect(put?.readOnly).toBe(false);
    expect(put?.doc.favorites.map((f) => f.name)).toEqual(["已有条目", "他端并发新增", "本地新建"]);
    expect(put?.doc.updated_at).toBeGreaterThanOrEqual(t0); // store 写回前统一盖戳
    // put 成功才 setState:本地整单与 put 一致
    expect(core.state.doc.favorites).toHaveLength(3);
    expect(core.state.ready).toBe(true);
    expect(core.state.busy).toBe(false);
  });

  it("⑤kvPut reject→false、状态保旧值、不自动重试", async () => {
    kvGetMock.mockResolvedValue(seedDoc([{ id: "keep-1", name: "旧值" }]));
    kvPutMock.mockRejectedValue(new Error("disk full"));
    const core = createFavoritesCore();
    core.setLoggedIn(true);
    await flush();

    const ok = await core.createFav("不会落盘");
    expect(ok).toBe(false);
    expect(kvPutMock).toHaveBeenCalledTimes(1); // 无自动重试
    expect(core.state.doc.favorites.map((f) => f.name)).toEqual(["旧值"]);
    expect(core.state.busy).toBe(false);
  });

  it("⑥data_changed key=favorites→refresh;key 不符→不刷", async () => {
    kvGetMock.mockResolvedValue(null);
    const core = createFavoritesCore();
    core.setLoggedIn(true);
    await flush();
    expect(kvGetMock).toHaveBeenCalledTimes(1);
    expect(dataChangedCb).toBeTypeOf("function");

    dataChangedCb?.(FAVORITES_KV_KEY);
    await flush();
    expect(kvGetMock).toHaveBeenCalledTimes(2);

    dataChangedCb?.("some_other_key");
    await flush();
    expect(kvGetMock).toHaveBeenCalledTimes(2);
  });
});

describe("useFavorites(编辑 op 补充面)", () => {
  it("addMembers:入参内去重计数;零新增/目标缺失→不回写", async () => {
    // kv.get 按次序演进(首拉→编辑前置取→落盘后再取,已含成员):
    const base = seedDoc([{ id: "f1", name: "项目A" }]);
    const withMember = seedDoc([{ id: "f1", name: "项目A", members: [{ device: "dev-1", path: "C:\\go" }] }]);
    kvGetMock.mockResolvedValueOnce(base).mockResolvedValueOnce(base).mockResolvedValue(withMember);
    kvPutMock.mockResolvedValue(undefined);
    const core = createFavoritesCore();
    core.setLoggedIn(true);
    await flush();

    const n1 = await core.addMembers("f1", [
      { device: "dev-1", path: "C:\\go" },
      { device: "dev-1", path: "C:\\go" },
    ]);
    expect(n1).toBe(1);
    expect(kvPutMock).toHaveBeenCalledTimes(1);

    const n2 = await core.addMembers("f1", [{ device: "dev-1", path: "C:\\go" }]);
    expect(n2).toBe(0);
    expect(kvPutMock).toHaveBeenCalledTimes(1); // 零新增不回写

    const n3 = await core.addMembers("missing", [{ device: "d", path: "p" }]);
    expect(n3).toBe(0);

    const put = parseFavorites(kvPutMock.mock.calls[0][1] as string);
    expect(put?.doc.favorites[0]?.members).toEqual([{ device: "dev-1", path: "C:\\go" }]);
    expect(core.state.doc.favorites[0]?.members).toHaveLength(1);
  });

  it("rename/removeMember/delete 走同一 mutate 面:put 成功才落状态+统一盖 updated_at", async () => {
    kvGetMock.mockResolvedValue(
      seedDoc([{ id: "f1", name: "旧名", members: [{ device: "d1", path: "C:\\a" }] }]),
    );
    kvPutMock.mockResolvedValue(undefined);
    const core = createFavoritesCore();
    core.setLoggedIn(true);
    await flush();

    expect(await core.renameFav("f1", "新名")).toBe(true);
    const putRename = parseFavorites(kvPutMock.mock.calls[0][1] as string);
    expect(putRename?.doc.favorites[0]?.name).toBe("新名");
    // renameFavorite 纯函数不动时间字段,updated_at 变新 = store 统一盖戳
    expect(putRename?.doc.updated_at).toBeGreaterThan(1_700_000_000_000);

    expect(await core.removeMember("f1", { device: "d1", path: "C:\\a" })).toBe(true);
    const putRemove = parseFavorites(kvPutMock.mock.calls[1][1] as string);
    expect(putRemove?.doc.favorites[0]?.members).toEqual([]);

    expect(await core.deleteFav("f1")).toBe(true);
    const putDelete = parseFavorites(kvPutMock.mock.calls[2][1] as string);
    expect(putDelete?.doc.favorites).toEqual([]);
    expect(core.state.doc.favorites).toEqual([]);
    expect(core.state.busy).toBe(false);
  });
});
