/// useFavListing 引擎行为测试(V3-3 Task 9,沿 favstore.test.ts 手法:
/// 仓库测试设施为 node 环境 vitest、无 hook 渲染能力,逻辑收口在
/// createFavFetchCore(React 无关引擎),测试直驱引擎钉行为,useFavListing
/// 只做薄接线):
/// ①快照离线成员初始即 offline 桶,daemonList 从不以该 (device,path) 调用
///   (Review Focus #3);②在线成员 resolve→ok 并入、loading 收敛;
/// ③reject→failed 桶(逐成员失败不波及他人,unavailableCount 面);
/// ④换 fav(新 id)/同 fav reloadTick 递增→旧应答整体丢弃(引擎世代守卫,
///   App.tsx 列表 seq 纪律同款);⑤设备快照软更新(fav+tick 不变):
///   pending→offline 翻桶且不重发、迟到应答按成员现状态丢弃,offline→
///   pending 翻桶并补发;已定成员(ok)不受 devices 变更影响(不重拉裁定);
/// ⑥同 fav+tick+devices 引用重复 sync 幂等(StrictMode 双跑安全)。
import { beforeEach, describe, expect, it, vi } from "vitest";

const listMock = vi.fn();

vi.mock("./daemon", () => ({
  daemonList: (...args: unknown[]) => listMock(...args),
}));

import type { FsEntry, Listing } from "./browse";
import type { DeviceInfo } from "./derived";
import type { FavMember, Favorite } from "./favorites";
import { unavailableCount } from "./mixedview";
import { createFavFetchCore } from "./favfetch";

/// 排干微任务(mock 应答链一个宏任务 tick 后全部收敛,favstore.test 同款)。
const flush = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

/// 手控应答闸(promise 外解)。
interface Gate {
  p: Promise<Listing>;
  resolve: (l: Listing) => void;
  reject: (e: unknown) => void;
}
function makeGate(): Gate {
  let resolve!: (l: Listing) => void;
  let reject!: (e: unknown) => void;
  const p = new Promise<Listing>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { p, resolve, reject };
}

/// 每次 daemonList 调用发一枚新闸(按调用序存入数组),供逐成员手控应答。
function gatePerCall(): Gate[] {
  const gates: Gate[] = [];
  listMock.mockImplementation(() => {
    const g = makeGate();
    gates.push(g);
    return g.p;
  });
  return gates;
}

const dev = (id: string, online: boolean): DeviceInfo => ({
  device_id: id,
  name: id,
  platform: "windows",
  online,
});
const mem = (device: string, path: string): FavMember => ({ device, path });
const favOf = (id: string, members: FavMember[]): Favorite => ({
  id,
  name: id,
  created_at: 1,
  members,
});
const ent = (name: string, path: string): FsEntry => ({
  name,
  path,
  size: 1,
  mtime_ms: 0,
  is_dir: false,
});
const listing = (...entries: FsEntry[]): Listing => ({ entries, truncated: false });

beforeEach(() => {
  listMock.mockReset();
});

describe("useFavListing 引擎(V3-3 Task 9)", () => {
  it("①快照离线成员初始即 offline 桶,daemonList 从不以该成员调用;在线成员 pending", () => {
    const gates = gatePerCall();
    const core = createFavFetchCore();
    core.sync(
      favOf("f1", [mem("dev-1", "C:\\a"), mem("dev-2", "C:\\b")]),
      [dev("dev-1", true), dev("dev-2", false)],
      0,
    );
    expect(core.snapshot.states.map((s) => s.status)).toEqual(["pending", "offline"]);
    expect(core.snapshot.states.every((s) => s.entries.length === 0)).toBe(true);
    expect(listMock).toHaveBeenCalledTimes(1); // 只发在线成员
    expect(listMock).toHaveBeenCalledWith("dev-1", "C:\\a");
    expect(listMock).not.toHaveBeenCalledWith("dev-2", "C:\\b");
    expect(core.snapshot.loading).toBe(true); // 任一成员在途
    gates[0]?.resolve(listing()); // 收尾:防悬挂闸泄漏进后续用例的断言面
  });

  it("②在线成员 resolve→ok 并入(entries 落地),loading 收敛 false", async () => {
    const gates = gatePerCall();
    const core = createFavFetchCore();
    core.sync(favOf("f1", [mem("dev-1", "C:\\a")]), [dev("dev-1", true)], 0);
    gates[0]?.resolve(listing(ent("x.txt", "C:\\a\\x.txt")));
    await flush();
    expect(core.snapshot.states).toEqual([
      { member: mem("dev-1", "C:\\a"), status: "ok", entries: [ent("x.txt", "C:\\a\\x.txt")] },
    ]);
    expect(core.snapshot.loading).toBe(false);
  });

  it("③reject→failed 桶(逐成员失败不波及他人)", async () => {
    const gates = gatePerCall();
    const core = createFavFetchCore();
    core.sync(
      favOf("f1", [mem("dev-1", "C:\\a"), mem("dev-2", "/b")]),
      [dev("dev-1", true), dev("dev-2", true)],
      0,
    );
    gates[1]?.reject(new Error("boom")); // dev-2 失败
    await flush();
    expect(core.snapshot.states.map((s) => s.status)).toEqual(["pending", "failed"]);
    expect(unavailableCount(core.snapshot.states)).toBe(1);
    gates[0]?.resolve(listing(ent("y.txt", "C:\\a\\y.txt"))); // dev-1 照常并入
    await flush();
    expect(core.snapshot.states.map((s) => s.status)).toEqual(["ok", "failed"]);
    expect(core.snapshot.loading).toBe(false);
  });

  it("④换 fav(新 id)→旧应答整体丢弃,新成员从快照重建;新世代应答照常落地", async () => {
    const gates = gatePerCall();
    const core = createFavFetchCore();
    core.sync(favOf("f1", [mem("dev-1", "C:\\a")]), [dev("dev-1", true)], 0);
    core.sync(favOf("f2", [mem("dev-1", "C:\\x")]), [dev("dev-1", true)], 0);
    gates[0]?.resolve(listing(ent("stale.txt", "C:\\a\\stale.txt"))); // f1 迟到应答
    await flush();
    expect(core.snapshot.states).toEqual([
      { member: mem("dev-1", "C:\\x"), status: "pending", entries: [] },
    ]);
    expect(core.snapshot.loading).toBe(true);
    gates[1]?.resolve(listing(ent("fresh.txt", "C:\\x\\fresh.txt"))); // f2 应答
    await flush();
    expect(core.snapshot.states).toEqual([
      { member: mem("dev-1", "C:\\x"), status: "ok", entries: [ent("fresh.txt", "C:\\x\\fresh.txt")] },
    ]);
  });

  it("④b 同 fav reloadTick 递增→旧应答丢弃、新请求发出并落地(刷新语义)", async () => {
    const gates = gatePerCall();
    const core = createFavFetchCore();
    const fav = favOf("f1", [mem("dev-1", "C:\\a")]);
    core.sync(fav, [dev("dev-1", true)], 0);
    core.sync(fav, [dev("dev-1", true)], 1);
    gates[0]?.resolve(listing(ent("stale.txt", "C:\\a\\stale.txt")));
    await flush();
    expect(core.snapshot.states[0]?.status).toBe("pending");
    expect(core.snapshot.states[0]?.entries).toEqual([]);
    expect(listMock).toHaveBeenCalledTimes(2);
    expect(listMock).toHaveBeenLastCalledWith("dev-1", "C:\\a");
    gates[1]?.resolve(listing(ent("fresh.txt", "C:\\a\\fresh.txt")));
    await flush();
    expect(core.snapshot.states[0]?.status).toBe("ok");
    expect(core.snapshot.states[0]?.entries).toEqual([ent("fresh.txt", "C:\\a\\fresh.txt")]);
  });

  it("⑤devices 快照软更新:pending→offline 翻桶且不重发,迟到应答按现状态丢弃", async () => {
    const gates = gatePerCall();
    const core = createFavFetchCore();
    const fav = favOf("f1", [mem("dev-1", "C:\\a")]);
    core.sync(fav, [dev("dev-1", true)], 0);
    expect(listMock).toHaveBeenCalledTimes(1);
    core.sync(fav, [dev("dev-1", false)], 0); // 仅 devices 换快照(fav+tick 不变)
    expect(core.snapshot.states[0]?.status).toBe("offline");
    expect(listMock).toHaveBeenCalledTimes(1); // 不重发
    expect(core.snapshot.loading).toBe(false);
    gates[0]?.resolve(listing(ent("x.txt", "C:\\a\\x.txt"))); // 软更新前的在途应答
    await flush();
    expect(core.snapshot.states[0]?.status).toBe("offline"); // 迟到应答丢弃
    expect(core.snapshot.states[0]?.entries).toEqual([]);
  });

  it("⑤b offline→devices 转在线→翻 pending 并补发请求", () => {
    const gates = gatePerCall();
    const core = createFavFetchCore();
    const fav = favOf("f1", [mem("dev-1", "C:\\a")]);
    core.sync(fav, [dev("dev-1", false)], 0);
    expect(core.snapshot.states[0]?.status).toBe("offline");
    expect(listMock).not.toHaveBeenCalled();
    core.sync(fav, [dev("dev-1", true)], 0);
    expect(core.snapshot.states[0]?.status).toBe("pending");
    expect(listMock).toHaveBeenCalledTimes(1);
    expect(listMock).toHaveBeenLastCalledWith("dev-1", "C:\\a");
    expect(core.snapshot.loading).toBe(true);
    gates[0]?.resolve(listing());
  });

  it("⑤c 已定成员(ok)不受 devices 变更影响:不重拉不翻桶(裁定)", async () => {
    listMock.mockResolvedValue(listing(ent("x.txt", "C:\\a\\x.txt")));
    const core = createFavFetchCore();
    const fav = favOf("f1", [mem("dev-1", "C:\\a")]);
    core.sync(fav, [dev("dev-1", true)], 0);
    await flush();
    expect(core.snapshot.states[0]?.status).toBe("ok");
    core.sync(fav, [dev("dev-1", false)], 0); // 设备后续转离线:ok 面保留
    expect(core.snapshot.states[0]?.status).toBe("ok");
    expect(listMock).toHaveBeenCalledTimes(1); // 不重拉
  });

  it("⑥同 fav+tick+devices 引用重复 sync 幂等(StrictMode 双跑安全)", () => {
    listMock.mockResolvedValue(listing());
    const core = createFavFetchCore();
    const fav = favOf("f1", [mem("dev-1", "C:\\a")]);
    const devices = [dev("dev-1", true)];
    core.sync(fav, devices, 0);
    core.sync(fav, devices, 0);
    expect(listMock).toHaveBeenCalledTimes(1);
    expect(core.snapshot.states).toHaveLength(1);
  });
});
