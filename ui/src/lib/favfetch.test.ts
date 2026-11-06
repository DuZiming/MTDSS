/// useFavListing 引擎行为测试(V3-3 Task 9;返工波 4 §16.4 扩 SWR 缓存,
/// 沿 favstore.test.ts 手法:仓库测试设施为 node 环境 vitest、无 hook 渲
/// 染能力,逻辑收口在 createFavFetchCore(React 无关引擎),测试直驱引擎
/// 钉行为,useFavListing 只做薄接线):
/// ①快照离线成员初始即 offline 桶,daemonList 从不以该 (device,path) 调用
///   (Review Focus #3);②在线成员 resolve→ok 并入、loading 收敛;
/// ③reject→failed 桶(逐成员失败不波及他人,unavailableCount 面);
/// ④换 fav(新 id)/同 fav reloadTick 递增→旧应答整体丢弃(引擎世代守卫,
///   App.tsx 列表 seq 纪律同款);⑤设备快照软更新(fav+tick 不变):
///   pending→offline 翻桶且不重发、迟到应答按成员现状态丢弃,offline→
///   pending 翻桶并补发;已定成员(ok)不受 devices 变更影响(不重拉裁定);
/// ⑥同 fav+tick+devices 引用重复 sync 幂等(StrictMode 双跑安全);
/// ⑦+ SWR 缓存(§16.4):命中即时种子(零 pending 整表瞬时,后台再校验)、
///   再校验差异落位(未变零扰动不订阅、变了 upsert、失败静默保旧)、
///   members 变更失效、离线重判、登出清场。
import { beforeEach, describe, expect, it, vi } from "vitest";

const listMock = vi.fn();

vi.mock("./daemon", () => ({
  daemonList: (...args: unknown[]) => listMock(...args),
}));

import type { FsEntry, Listing } from "./browse";
import type { DeviceInfo } from "./derived";
import type { FavMember, Favorite } from "./favorites";
import { unavailableCount } from "./mixedview";
import { clearFavCache, createFavFetchCore } from "./favfetch";

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
  clearFavCache(); // 模块级 SWR 缓存按例清场(防用例间串场)
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

describe("SWR 缓存(返工波 4 §16.4:重进瞬时/再校验差异/失效清场)", () => {
  it("⑦缓存命中即时种子:重进同快捷首读即 ok 整表(零 pending),在线成员后台再校验", async () => {
    // 预热:首次进入渐进拉取并落缓存。
    const gates = gatePerCall();
    const fav = favOf("f1", [mem("dev-1", "C:\\a")]);
    const devices = [dev("dev-1", true)];
    const core1 = createFavFetchCore();
    core1.sync(fav, devices, 0);
    gates[0]?.resolve(listing(ent("a.txt", "C:\\a\\a.txt")));
    await flush();
    expect(core1.snapshot.states[0]?.status).toBe("ok");
    // 重进(引擎重挂载 = 新 core,世代重建):首读(sync 同步段内)即
    // 缓存种子——ok + entries 落地、零 pending、loading false,不整段等待。
    const core2 = createFavFetchCore();
    const g2 = gatePerCall(); // 再校验请求照发(后台静默校验)
    core2.sync(fav, devices, 0);
    expect(core2.snapshot.states).toEqual([
      { member: mem("dev-1", "C:\\a"), status: "ok", entries: [ent("a.txt", "C:\\a\\a.txt")] },
    ]);
    expect(core2.snapshot.loading).toBe(false);
    expect(listMock).toHaveBeenCalledTimes(2); // 预热 1 + 再校验 1
    expect(listMock).toHaveBeenLastCalledWith("dev-1", "C:\\a");
    // 再校验同内容应答:未变零扰动(见 ⑧b)。
    g2[0]?.resolve(listing(ent("a.txt", "C:\\a\\a.txt")));
    await flush();
    expect(core2.snapshot.states[0]?.status).toBe("ok");
  });

  it("⑦b 缓存成员设备现已离线:重判 offline 翻桶(离线判定恒以 devices 快照为准),不发 list", () => {
    gatePerCall();
    const fav = favOf("f1", [mem("dev-1", "C:\\a")]);
    const core1 = createFavFetchCore();
    listMock.mockResolvedValueOnce(listing(ent("a.txt", "C:\\a\\a.txt")));
    core1.sync(fav, [dev("dev-1", true)], 0);
    const core2 = createFavFetchCore();
    core2.sync(fav, [dev("dev-1", false)], 0);
    expect(core2.snapshot.states).toEqual([
      { member: mem("dev-1", "C:\\a"), status: "offline", entries: [] },
    ]);
    expect(core2.snapshot.loading).toBe(false);
    expect(listMock).toHaveBeenCalledTimes(1); // 离线成员从不发 list(种子后不再校验)
  });

  it("⑧再校验差异落位:变了 upsert;未变不订阅不落(快照引用不动,零重排)", async () => {
    // 预热缓存(entries v1)。
    const gates = gatePerCall();
    const fav = favOf("f1", [mem("dev-1", "C:\\a")]);
    const devices = [dev("dev-1", true)];
    const core1 = createFavFetchCore();
    core1.sync(fav, devices, 0);
    gates[0]?.resolve(listing(ent("a.txt", "C:\\a\\a.txt")));
    await flush();
    // 重进:缓存种子 + 再校验闸。
    const g2 = gatePerCall();
    const core2 = createFavFetchCore();
    core2.sync(fav, devices, 0);
    let notifies = 0;
    core2.subscribe(() => {
      notifies += 1;
    });
    const seeded = core2.snapshot;
    // (a)未变应答:不落不订阅——快照引用原样,订阅回调零触发。
    g2[0]?.resolve(listing(ent("a.txt", "C:\\a\\a.txt")));
    await flush();
    expect(core2.snapshot).toBe(seeded);
    expect(notifies).toBe(0);
    // (b)变了应答:差异 upsert(新 entries 落地)。订阅计数在刷新世代
    // 同步之后清零重计(硬复位种子 commit 本身通知一次,不属再校验面;
    // 订阅仍挂着,不重挂)。
    const g3 = gatePerCall();
    core2.sync(fav, devices, 1); // 刷新世代(重进语义同款,缓存仍在)
    notifies = 0;
    g3[0]?.resolve(listing(ent("a.txt", "C:\\a\\a.txt"), ent("b.txt", "C:\\a\\b.txt")));
    await flush();
    expect(core2.snapshot.states).toEqual([
      {
        member: mem("dev-1", "C:\\a"),
        status: "ok",
        entries: [ent("a.txt", "C:\\a\\a.txt"), ent("b.txt", "C:\\a\\b.txt")],
      },
    ]);
    expect(notifies).toBe(1);
    // 差异已同步进缓存(再次重进即新 entries 瞬时)。
    const core3 = createFavFetchCore();
    core3.sync(fav, devices, 0);
    expect(core3.snapshot.states[0]?.entries).toHaveLength(2);
  });

  it("⑧b 再校验失败:静默保旧(stale 保留,不闪失败面)", async () => {
    const gates = gatePerCall();
    const fav = favOf("f1", [mem("dev-1", "C:\\a")]);
    const devices = [dev("dev-1", true)];
    const core1 = createFavFetchCore();
    core1.sync(fav, devices, 0);
    gates[0]?.resolve(listing(ent("a.txt", "C:\\a\\a.txt")));
    await flush();
    const g2 = gatePerCall();
    const core2 = createFavFetchCore();
    core2.sync(fav, devices, 0);
    expect(core2.snapshot.states[0]?.status).toBe("ok"); // 缓存种子先呈现
    g2[0]?.reject(new Error("boom")); // 再校验失败
    await flush();
    expect(core2.snapshot.states).toEqual([
      { member: mem("dev-1", "C:\\a"), status: "ok", entries: [ent("a.txt", "C:\\a\\a.txt")] },
    ]);
    expect(core2.snapshot.loading).toBe(false);
  });

  it("⑨members 变更失效:同 id 快捷签名不符 → 缓存即弃回快照种子,陈旧条目不呈现", async () => {
    // 预热:单成员 ok 落缓存。
    const gates = gatePerCall();
    const fav1 = favOf("f1", [mem("dev-1", "C:\\a")]);
    const core1 = createFavFetchCore();
    core1.sync(fav1, [dev("dev-1", true), dev("dev-2", true)], 0);
    gates[0]?.resolve(listing(ent("a.txt", "C:\\a\\a.txt")));
    await flush();
    // 管理屏加成员(fav 对象换新、tick 不变):sync 检出签名不符 → 硬复位,
    // 缓存弃(新成员 pending 首拉,原成员也回快照种子重拉——签名不符不 seed)。
    const g2 = gatePerCall();
    const core2 = createFavFetchCore();
    const fav2 = favOf("f1", [mem("dev-1", "C:\\a"), mem("dev-2", "C:\\b")]);
    core2.sync(fav2, [dev("dev-1", true), dev("dev-2", true)], 0);
    expect(core2.snapshot.states.map((s) => s.status)).toEqual(["pending", "pending"]);
    expect(core2.snapshot.states.every((s) => s.entries.length === 0)).toBe(true);
    expect(listMock).toHaveBeenCalledTimes(3); // 预热 1 + 重拉 2
    g2[0]?.resolve(listing(ent("a.txt", "C:\\a\\a.txt")));
    g2[1]?.resolve(listing(ent("b.txt", "C:\\b\\b.txt")));
    await flush();
    expect(core2.snapshot.states.map((s) => s.status)).toEqual(["ok", "ok"]);
  });

  it("⑨b 同 id 同 members(fav 对象换新,如他端改它快捷)不误判失效:缓存仍命中", async () => {
    const gates = gatePerCall();
    const fav1 = favOf("f1", [mem("dev-1", "C:\\a")]);
    const core1 = createFavFetchCore();
    core1.sync(fav1, [dev("dev-1", true)], 0);
    gates[0]?.resolve(listing(ent("a.txt", "C:\\a\\a.txt")));
    await flush();
    const core2 = createFavFetchCore();
    core2.sync(favOf("f1", [mem("dev-1", "C:\\a")]), [dev("dev-1", true)], 0); // 同签名新对象
    expect(core2.snapshot.states[0]?.status).toBe("ok"); // 缓存命中
    expect(core2.snapshot.states[0]?.entries).toHaveLength(1);
  });

  it("⑩登出清场:clearFavCache() 后重进同快捷回快照种子(不瞬时复用)", async () => {
    const gates = gatePerCall();
    const fav = favOf("f1", [mem("dev-1", "C:\\a")]);
    const core1 = createFavFetchCore();
    core1.sync(fav, [dev("dev-1", true)], 0);
    gates[0]?.resolve(listing(ent("a.txt", "C:\\a\\a.txt")));
    await flush();
    clearFavCache(); // App 登出/被移出路径
    const g2 = gatePerCall();
    const core2 = createFavFetchCore();
    core2.sync(fav, [dev("dev-1", true)], 0);
    expect(core2.snapshot.states.map((s) => s.status)).toEqual(["pending"]); // 回快照种子
    expect(core2.snapshot.states[0]?.entries).toEqual([]);
    g2[0]?.resolve(listing());
  });
});
