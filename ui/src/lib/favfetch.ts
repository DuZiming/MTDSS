/// 快捷目录主视图的成员拉取引擎与 hook(V3-3 Task 9;返工波 4 §16.4 扩
/// SWR 缓存):
///
/// 渐进扇出:对 fav 全部成员并行 daemonList,应答逐成员到达、逐次 upsert,
/// 行经 FavView 分组管线渐进出现(不等齐)。快照分桶(spec §5.2):
/// devices 快照判离线的成员**初始即 offline 桶,从不发 list**;其余 pending。
///
/// 世代守卫(App.tsx 列表 seq 纪律同款):fav.id+reloadTick+成员签名 任一
/// 变化 = 硬复位,世代 +1,旧世代迟到应答整体丢弃(导航换快捷/刷新/管理
/// 屏改成员不串台)。devices 快照变化 = 软更新(同 fav+tick+签名):只对
/// 未定成员(pending/offline)重判翻桶——pending→offline 翻桶,其迟到应答
/// 按「成员现状态非 pending」就地丢弃;offline→pending 翻桶并补发;**已定
/// 成员(ok/failed)不动、不重拉**(裁定:devices 变更不为 ok 成员重发请求)。
/// 软更新不推世代:设备仍在线的成员其既有在途应答仍然有效,若整体弃之
/// (世代 +1)会使其永远 pending——现状态守卫已覆盖真正该弃的场景。
///
/// SWR 缓存(返工波 4 §16.4,分类恒定后的无感化收窄面):模块级按 fav
/// 缓存各成员最近一次 ok 应答——重进同快捷(引擎重挂载世代重建)先以
/// 缓存即时种子(命中成员直接 ok 落行,零 pending,整表瞬时呈现),在线
/// 成员后台再校验,应答**差异落位**(entries 未变 = 不落不订阅,零重排;
/// 变了 = 差异 upsert;再校验失败 = 静默保旧,不闪失败面)。失效两路:
/// ①members 签名变化(管理屏编辑/他端同步,sync 检测)→ 该 fav 缓存即弃
/// 回快照种子;②登出/被移出 → clearFavCache() 清场(App 在登录态事件处
/// 调用)。缓存仅内存不落盘,离线判定恒以 devices 快照为准(缓存成员设备
/// 现已离线 → 照常 offline 翻桶)。
///
/// 实现形态:逻辑收口在 createFavFetchCore(React 无关引擎,favfetch.test
/// 直驱钉行为,T4 favstore 先例),useFavListing 只做 useState/useEffect
/// 薄接线;首帧快照由 seedCachedSnapshot 纯函数给出(缓存命中整表、空缓存
/// 退 seedSnapshot 同形;SSR/首绘确定性且不发请求)——返工波 4 X3 起
/// useFavListing 初始 useState 直驱本函数,缓存命中首绘即整表(消掉旧
/// seedSnapshot 种子路径在缓存命中时的一帧 pending 闪面)。
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

/// ---- SWR 缓存(§16.4,模块级:跨 FavView 重挂载存活)----
/// favId → 各成员最近一次 ok 应答(states 仅含 ok 成员)+ 成员签名。
/// 签名 = memberKey 串接(␁ 分隔;memberKey 自身 NUL 分隔不冲突)——
/// 签名不符 = members 变更(管理屏编辑/他端同步),缓存即失效。
interface FavCacheEntry {
  sig: string;
  states: MemberState[];
}
const favCache = new Map<string, FavCacheEntry>();

const membersSig = (members: FavMember[]): string =>
  members.map((m) => memberKey(m)).join("\u0001");

/// 登出/被移出清场(模块缓存跨会话存活,必须显式清;App 在登录态事件
/// 处调用——换账号后旧账号的目录内容绝不可瞬时复用)。
export function clearFavCache(): void {
  favCache.clear();
}

/// 首帧缓存种子(§16.4,React 无关纯函数,读模块缓存):签名命中 = 缓存
/// ok 成员即时落位(entries 用缓存,零 pending,整表瞬时);成员设备现已
/// 离线 → 照常 offline 翻桶(离线判定恒以 devices 快照为准);未命中成员
/// pending。签名不符(members 变更)→ 该 fav 缓存即弃,退回快照种子。
/// 空缓存(SSR/首登/登出清场后)即 seedSnapshot 同形,不发请求——
/// useFavListing 初始 useState 与引擎 sync 硬复位同驱本函数(种子同源)。
export function seedCachedSnapshot(fav: Favorite, devices: DeviceInfo[]): FavListingSnapshot {
  const cached = favCache.get(fav.id);
  if (cached == null || cached.sig !== membersSig(fav.members)) {
    favCache.delete(fav.id);
    return seedSnapshot(fav, devices);
  }
  const online =
    (d: string): boolean =>
      devices.find((x) => x.device_id === d)?.online ?? false;
  const states = fav.members.map((member) => {
    if (!online(member.device)) return { member, status: "offline" as const, entries: [] };
    const hit = cached.states.find(
      (s) => s.status === "ok" && memberKey(s.member) === memberKey(member),
    );
    return hit != null
      ? { member, status: "ok" as const, entries: hit.entries }
      : { member, status: "pending" as const, entries: [] };
  });
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
  /// 世代:fav.id+reloadTick+成员签名 硬复位时 +1;旧世代应答整体丢弃。
  let gen = 0;
  /// 当前加载键(fav.id \0 reloadTick);同键 sync = 软更新(devices 重判)。
  let key: string | null = null;
  /// 当前 fav 的成员签名(sync 检测 members 变更:同 id 快捷管理屏编辑后
  /// fav 对象换新,键不变签名变 → 硬复位重扇出,§16.4 失效钩子①)。
  let curSig: string | null = null;
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

  /// 条目差异判定(§16.4 再校验面):逐条逐字段比——全同 = 未变(不落
  /// 不订阅,零重排)。FsEntry 五字段全量(库层无派生字段)。
  const sameEntries = (a: FsEntry[], b: FsEntry[]): boolean =>
    a.length === b.length &&
    a.every((e, i) => {
      const o = b[i] as FsEntry;
      return (
        e.path === o.path &&
        e.name === o.name &&
        e.size === o.size &&
        e.mtime_ms === o.mtime_ms &&
        e.is_dir === o.is_dir
      );
    });

  /// 缓存落位(ok 应答写入模块缓存;仅 ok 态,失败/离线不写=保旧)。
  const cachePut = (favId: string, member: FavMember, entries: FsEntry[]): void => {
    const sig = curSig ?? "";
    const cur = favCache.get(favId);
    const okState: MemberState = { member, status: "ok", entries };
    if (cur == null || cur.sig !== sig) {
      favCache.set(favId, { sig, states: [okState] });
    } else {
      favCache.set(favId, { sig, states: memberStateUpsert(cur.states, member, "ok", entries) });
    }
  };

  /// 单成员应答落态:世代守卫(硬复位后旧应答弃)+ 分状态落位——
  /// pending 应答照旧 upsert(pending→ok/failed);已 ok 成员的后台再校验
  /// 应答(§16.4 SWR)按差异落位:未变不落(订阅不触发,零重排),变了
  /// 差异 upsert,失败静默保旧(stale 保留,不闪失败面)。
  const applyMember = (
    myGen: number,
    favId: string,
    member: FavMember,
    status: "ok" | "failed",
    entries: FsEntry[],
  ): void => {
    if (myGen !== gen) return;
    const cur = snap.states.find((s) => memberKey(s.member) === memberKey(member));
    if (cur == null) return;
    if (cur.status === "pending") {
      commit(memberStateUpsert(snap.states, member, status, entries));
      if (status === "ok") cachePut(favId, member, entries);
      return;
    }
    if (cur.status === "ok" && status === "ok") {
      if (sameEntries(cur.entries, entries)) return;
      commit(memberStateUpsert(snap.states, member, "ok", entries));
      cachePut(favId, member, entries);
    }
  };

  /// 单成员拉取:resolve→ok(entries 并入);reject→failed(entries 恒空)。
  /// 逐成员独立(并行 promise,一人失败不波及他人)。
  const fetchMember = (member: FavMember, myGen: number, favId: string): void => {
    daemonList(member.device, member.path)
      .then((l) => applyMember(myGen, favId, member, "ok", l.entries))
      .catch(() => applyMember(myGen, favId, member, "failed", []));
  };

  /// 缓存种子(§16.4):直接驱动模块级 seedCachedSnapshot(与 useFavListing
  /// 初始 useState 同源——同 fav 同 devices 种出同形快照)。
  const seedWithCache = (fav: Favorite, devices: DeviceInfo[]): MemberState[] =>
    seedCachedSnapshot(fav, devices).states;

  const sync = (fav: Favorite, devices: DeviceInfo[], reloadTick: number): void => {
    const k = `${fav.id}\u0000${reloadTick}`;
    const sig = membersSig(fav.members);
    if (k === key && sig === curSig) {
      // 软更新:仅 devices 快照变化(fav+tick+members 不变)。未定成员重判
      // 翻桶;已定(ok/failed)成员不动(裁定:不重拉不翻桶)。
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
          fetchMember(s.member, gen, fav.id); // 补发(现世代)
          return { ...s, status: "pending" as const };
        }
        return s;
      });
      if (changed) commit(next);
      return;
    }
    // 硬复位:fav/刷新世代/成员签名变化 → 快照重建 + 并行扇出。在线成员
    // 全部(重)拉取:pending 首拉;缓存 ok 成员后台再校验(§16.4 SWR,
    // 差异落位未变零扰动);离线成员从不发 list。
    key = k;
    curSig = sig;
    gen += 1;
    const myGen = gen;
    const seeded = seedWithCache(fav, devices);
    commit(seeded);
    for (const s of seeded) {
      if (s.status === "pending" || s.status === "ok") fetchMember(s.member, myGen, fav.id);
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
/// seedCachedSnapshot 纯给出(缓存命中首绘即整表,零 pending 帧;空缓存/
/// 离线成员退 seedSnapshot 同形——离线成员即时入桶、不发请求,SSR/首绘
/// 确定性),effects 里 sync 驱动 + 订阅镜像。fav.id 变更(导航换快捷)
/// 由 App 以 key 重挂载收口;刷新经 reloadTick。
export function useFavListing(
  fav: Favorite,
  devices: DeviceInfo[],
  reloadTick: number,
): FavListingSnapshot {
  const core = useMemo(() => createFavFetchCore(), []);
  const [snap, setSnap] = useState<FavListingSnapshot>(() => seedCachedSnapshot(fav, devices));
  // 先挂订阅再 sync(声明序):sync 落态不漏镜像;回调现场读快照
  // (setState 必须显式入参,不能直接当零参回调)。
  useEffect(() => core.subscribe(() => setSnap(core.snapshot)), [core]);
  useEffect(() => {
    core.sync(fav, devices, reloadTick);
  }, [core, fav, devices, reloadTick]);
  return snap;
}
