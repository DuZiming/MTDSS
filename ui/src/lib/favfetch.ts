/// 常用目录混合视图的成员拉取引擎与 hook(V3-3 Task 9):
///
/// 渐进扇出(mixedview 渐进渲染裁定的拉取侧):对 fav 全部成员并行
/// daemonList,应答逐成员到达、逐次 upsert,行经 FavView 的 mixedRows
/// 管线渐进出现(不等齐)。快照分桶(spec §5.2):devices 快照判离线的
/// 成员**初始即 offline 桶,从不发 list**;其余 pending。
///
/// 世代守卫(App.tsx 列表 seq 纪律同款):fav.id+reloadTick 变化 = 硬
/// 复位,世代 +1,旧世代迟到应答整体丢弃(导航换常用/刷新不串台)。
/// devices 快照变化 = 软更新(同 fav+tick):只对未定成员(pending/
/// offline)重判翻桶——pending→offline 翻桶,其迟到应答按「成员现状态
/// 非 pending」就地丢弃;offline→pending 翻桶并补发;**已定成员(ok/
/// failed)不动、不重拉**(裁定:devices 变更不为 ok 成员重发请求)。
/// 软更新不推世代:设备仍在线的成员其既有在途应答仍然有效,若整体弃之
/// (世代 +1)会使其永远 pending——现状态守卫已覆盖真正该弃的场景。
///
/// 已知边界:同 id 常用的 members 变更(管理屏编辑)不触发重拉,随
/// reloadTick/重进收口(T13 管理屏接线时处置)。
///
/// 实现形态:逻辑收口在 createFavFetchCore(React 无关引擎,favfetch.test
/// 直驱钉行为,T4 favstore 先例),useFavListing 只做 useState/useEffect
/// 薄接线;首帧快照由 seedSnapshot 纯函数给出(与引擎 sync 种子同源,
/// SSR/首绘确定性且不发请求)。
import { useEffect, useMemo, useState } from "react";

import { daemonList } from "./daemon";
import type { DeviceInfo } from "./derived";
import { memberKey, type FavMember, type Favorite } from "./favorites";
import { memberStateUpsert, offlineMembers, type MemberState } from "./mixedview";
import type { FsEntry } from "./browse";

/// 拉取快照:states + loading(= 任一成员 pending,派生量不单独持有)。
export interface FavListingSnapshot {
  states: MemberState[];
  loading: boolean;
}

/// 初始快照(纯函数,不发请求):按 devices 快照分桶 offline/pending,
/// 序 = 成员添加序;loading = 任一成员 pending。
export function seedSnapshot(fav: Favorite, devices: DeviceInfo[]): FavListingSnapshot {
  const states = offlineMembers(
    fav.members,
    (d) => devices.find((x) => x.device_id === d)?.online ?? false,
  );
  return { states, loading: states.some((s) => s.status === "pending") };
}

/// React 无关拉取引擎(favfetch.test.ts 的直驱面):快照经 subscribe 分发,
/// sync(fav, devices, reloadTick) 为唯一驱动入口(hook effect 调用;
/// 幂等,同参重入无副作用——StrictMode 双跑安全)。
export interface FavFetchCore {
  /// 当前快照(每次变更替换新对象,比较引用即可判变化)。
  readonly snapshot: FavListingSnapshot;
  /// 订阅快照变化 → 解订函数。
  subscribe(cb: () => void): () => void;
  sync(fav: Favorite, devices: DeviceInfo[], reloadTick: number): void;
}

/// 铸造拉取引擎:每组件实例一个(useFavListing useMemo 持有),测试亦可
/// 独立铸造直驱。
export function createFavFetchCore(): FavFetchCore {
  let snap: FavListingSnapshot = { states: [], loading: false };
  /// 世代:fav.id+reloadTick 硬复位时 +1;旧世代应答整体丢弃。
  let gen = 0;
  /// 当前加载键(fav.id \0 reloadTick);同键 sync = 软更新(devices 重判)。
  let key: string | null = null;
  const subs = new Set<() => void>();

  /// 落快照并分发(loading 恒由 states 现状派生)。
  const commit = (states: MemberState[]): void => {
    snap = { states, loading: states.some((s) => s.status === "pending") };
    for (const cb of subs) cb();
  };

  const isOnlineIn =
    (devices: DeviceInfo[]) =>
    (d: string): boolean =>
      devices.find((x) => x.device_id === d)?.online ?? false;

  /// 单成员应答落态:世代守卫(硬复位后旧应答弃)+ 成员现状态守卫
  /// (软更新翻桶 offline 后的迟到应答弃)。双守卫后 upsert。
  const applyMember = (
    myGen: number,
    member: FavMember,
    status: "ok" | "failed",
    entries: FsEntry[],
  ): void => {
    if (myGen !== gen) return;
    const cur = snap.states.find((s) => memberKey(s.member) === memberKey(member));
    if (cur == null || cur.status !== "pending") return;
    commit(memberStateUpsert(snap.states, member, status, entries));
  };

  /// 单成员拉取:resolve→ok(entries 并入);reject→failed(entries 恒空)。
  /// 逐成员独立(并行 promise,一人失败不波及他人)。
  const fetchMember = (member: FavMember, myGen: number): void => {
    daemonList(member.device, member.path)
      .then((l) => applyMember(myGen, member, "ok", l.entries))
      .catch(() => applyMember(myGen, member, "failed", []));
  };

  const sync = (fav: Favorite, devices: DeviceInfo[], reloadTick: number): void => {
    const k = `${fav.id}\u0000${reloadTick}`;
    if (k === key) {
      // 软更新:仅 devices 快照变化(fav+tick 不变)。未定成员重判翻桶;
      // 已定(ok/failed)成员不动(裁定:不重拉不翻桶)。
      const online = isOnlineIn(devices);
      let changed = false;
      const next = snap.states.map((s) => {
        if (s.status === "ok" || s.status === "failed") return s;
        const now = online(s.member.device);
        if (s.status === "pending" && !now) {
          changed = true;
          return { ...s, status: "offline" as const }; // 迟到应答由 applyMember 弃
        }
        if (s.status === "offline" && now) {
          changed = true;
          fetchMember(s.member, gen); // 补发(现世代)
          return { ...s, status: "pending" as const };
        }
        return s;
      });
      if (changed) commit(next);
      return;
    }
    // 硬复位:fav 或刷新世代变化 → 快照重建 + 并行扇出。
    key = k;
    gen += 1;
    const myGen = gen;
    const seeded = seedSnapshot(fav, devices);
    commit(seeded.states);
    for (const s of seeded.states) {
      if (s.status === "pending") fetchMember(s.member, myGen);
    }
  };

  return {
    get snapshot() {
      return snap;
    },
    subscribe(cb: () => void): () => void {
      subs.add(cb);
      return () => {
        subs.delete(cb);
      };
    },
    sync,
  };
}

/// useFavListing(Task 9 brief 接口):每组件实例一引擎;首帧快照由
/// seedSnapshot 纯给出(离线成员即时入桶、不发请求——SSR/首绘确定性),
/// effects 里 sync 驱动 + 订阅镜像。fav.id 变更(导航换常用)由 App 以
/// key 重挂载收口;刷新经 reloadTick。
export function useFavListing(
  fav: Favorite,
  devices: DeviceInfo[],
  reloadTick: number,
): FavListingSnapshot {
  const core = useMemo(() => createFavFetchCore(), []);
  const [snap, setSnap] = useState<FavListingSnapshot>(() => seedSnapshot(fav, devices));
  // 先挂订阅再 sync(声明序):sync 落态不漏镜像;回调现场读快照
  // (setState 必须显式入参,不能直接当零参回调)。
  useEffect(() => core.subscribe(() => setSnap(core.snapshot)), [core]);
  useEffect(() => {
    core.sync(fav, devices, reloadTick);
  }, [core, fav, devices, reloadTick]);
  return snap;
}
