/// 常用目录 store(V3-3 Task 4):账号级 KV 整单 LWW 的前端收敛面,
/// 供 T8 起 UI 组件消费(useFavorites)。
///
/// 同步模型(spec 2026-11-05-mtdss-v3p3-favorites §3):登录即订阅
/// daemon://data_changed(key=favorites)并首拉;编辑 op 统一走私函
/// mutate「kv.get 取最新 → parse 前向守卫 → 纯函数改 → kv.put 整单回写」,
/// put 成功才 setState;put 失败状态保旧值、返回 false、不自动重试。
/// updated_at 由 mutate 写回前统一盖 Date.now()(纯函数不动时间字段,
/// createFavorite 的 now 参数仅作新条目 created_at)。前向守卫:解析出
/// v>1 → readOnly,拒绝编辑且绝不回写(防旧版覆盖新版数据)。
///
/// 实现形态:逻辑全部收口在 createFavoritesCore(React 无关引擎,持有
/// 状态与订阅;仓库测试设施无 hook 渲染能力,favstore.test.ts 直驱引擎
/// 钉行为),useFavorites 只做 useState/useEffect 薄接线。
import { useEffect, useMemo, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";

import { listenDataChanged, kvGet, kvPut } from "./daemon";
import {
  addMembers as addMembersFav,
  createFavorite,
  EMPTY_FAVS,
  FAVORITES_KV_KEY,
  parseFavorites,
  removeFavorite,
  removeMember as removeMemberFav,
  renameFavorite,
  type FavMember,
  type FavoritesDoc,
} from "./favorites";

/// store 状态面:doc=整单文档(初始/登出为 EMPTY);readOnly=前向守卫
/// (v>1 或解析失败,UI 禁编辑入口);ready=已完成至少一次成功加载
/// (首拉或一次成功编辑);busy=一次编辑 mutate 在途(UI 禁编辑按钮)。
export interface FavStoreState {
  doc: FavoritesDoc;
  readOnly: boolean;
  ready: boolean;
  busy: boolean;
}

/// 初始态(未登录/登出复位到此)。
export const FAVS_INITIAL: FavStoreState = {
  doc: EMPTY_FAVS,
  readOnly: false,
  ready: false,
  busy: false,
};

/// React 无关 store 引擎(favstore.test.ts 的直驱面):状态快照经
/// subscribe 分发,登录态由 setLoggedIn 翻转驱动订阅与首拉。
export interface FavoritesCore {
  /// 当前状态快照(每次变更替换新对象,比较引用即可判变化)。
  readonly state: FavStoreState;
  /// 订阅状态变化 → 解订函数。
  subscribe(cb: () => void): UnlistenFn;
  /// 登录态翻转:true → 复位+订阅 data_changed+首拉;false → 解订+复位。
  setLoggedIn(loggedIn: boolean): void;
  /// 手动重拉(服务端为准整单替换;失败状态保旧值)。
  refresh(): Promise<void>;
  createFav(name: string): Promise<boolean>;
  renameFav(id: string, name: string): Promise<boolean>;
  deleteFav(id: string): Promise<boolean>;
  addMembers(id: string, members: FavMember[]): Promise<number>;
  removeMember(id: string, member: FavMember): Promise<boolean>;
}

/// 铸造 store 引擎:每组件实例一个(useFavorites useMemo 持有),测试
/// 亦可独立铸造直驱。
export function createFavoritesCore(): FavoritesCore {
  let state: FavStoreState = FAVS_INITIAL;
  let loggedIn = false;
  /// 登录世代:每次翻转 +1,旧世代的在途首拉/编辑落态与迟到订阅一律
  /// 丢弃(登出后的迟到 put 结果不得覆盖复位态,重登后的新会话态同理)。
  let gen = 0;
  let unlistenData: UnlistenFn | null = null;
  const subs = new Set<() => void>();

  const setState = (patch: Partial<FavStoreState>): void => {
    state = { ...state, ...patch };
    for (const cb of subs) cb();
  };

  /// 世代守卫下的落态:翻转后旧世代结果丢弃。
  const apply = (myGen: number, patch: Partial<FavStoreState>): void => {
    if (myGen === gen) setState(patch);
  };

  /// 重拉(服务端为准整单替换):null(无键)= 合法空单;失败(离线/
  /// 未登录)状态保旧值,等下一次 data_changed/登录翻转再拉。
  const refresh = async (): Promise<void> => {
    const myGen = gen;
    if (!loggedIn) return;
    try {
      const raw = await kvGet(FAVORITES_KV_KEY);
      const parsed = parseFavorites(raw);
      apply(
        myGen,
        parsed === null
          ? { doc: EMPTY_FAVS, readOnly: false, ready: true }
          : { doc: parsed.doc, readOnly: parsed.readOnly, ready: true },
      );
    } catch {
      // 拉取失败:保旧值,不置 ready(等待下次信号)
    }
  };

  /// 订阅 data_changed:只认 favorites 键,其余键(未来其他账号 KV)
  /// 不触发重拉。迟到落地(世代已翻)立即解订。
  const subscribeData = (): void => {
    const myGen = gen;
    void listenDataChanged((key) => {
      if (key === FAVORITES_KV_KEY) void refresh();
    }).then((fn) => {
      if (myGen !== gen) fn();
      else unlistenData = fn;
    });
  };

  const setLoggedIn = (on: boolean): void => {
    loggedIn = on;
    gen += 1;
    unlistenData?.();
    unlistenData = null;
    setState({ doc: EMPTY_FAVS, readOnly: false, ready: false, busy: false }); // 换会话先复位
    if (on) {
      subscribeData();
      void refresh();
    }
  };

  /// 编辑 op 统一私函:前置 kv.get 必取最新(他端可能刚改)→ parse →
  /// readOnly 拒(绝不 kvPut)→ fn 改(null=无可写变更,不回写)→ 盖
  /// updated_at → kv.put 整单 → put 成功才 setState;失败状态保旧值、
  /// 返回 null、不自动重试。busy 门:在途编辑拒绝重入(双击/并发双发)。
  const mutate = async (
    fn: (doc: FavoritesDoc) => FavoritesDoc | null,
  ): Promise<FavoritesDoc | null> => {
    if (!loggedIn || state.busy) return null;
    const myGen = gen;
    setState({ busy: true });
    try {
      const raw = await kvGet(FAVORITES_KV_KEY);
      const parsed = parseFavorites(raw);
      if (parsed !== null && parsed.readOnly) {
        apply(myGen, { readOnly: true }); // 前向守卫: v>1 只读
        return null;
      }
      const next = fn(parsed === null ? EMPTY_FAVS : parsed.doc);
      if (next === null) return null;
      next.updated_at = Date.now(); // 写回前统一盖时间戳
      await kvPut(FAVORITES_KV_KEY, JSON.stringify(next));
      apply(myGen, { doc: next, readOnly: false, ready: true }); // put 成功才落状态
      return next;
    } catch {
      return null; // kv.get/kv.put 失败:保旧值,不重试
    } finally {
      apply(myGen, { busy: false });
    }
  };

  const createFav = async (name: string): Promise<boolean> =>
    (await mutate((doc) => createFavorite(doc, name, Date.now()))) !== null;

  const renameFav = async (id: string, name: string): Promise<boolean> =>
    (await mutate((doc) => renameFavorite(doc, id, name))) !== null;

  const deleteFav = async (id: string): Promise<boolean> =>
    (await mutate((doc) => removeFavorite(doc, id))) !== null;

  const removeMemberOp = async (id: string, member: FavMember): Promise<boolean> =>
    (await mutate((doc) => removeMemberFav(doc, id, member))) !== null;

  const addMembersOp = async (id: string, members: FavMember[]): Promise<number> => {
    let added = 0;
    const next = await mutate((doc) => {
      const r = addMembersFav(doc, id, members);
      added = r.added;
      return r.added === 0 ? null : r.doc; // 零新增(全重复/目标缺失)→ 不回写
    });
    return next === null ? 0 : added;
  };

  return {
    get state() {
      return state;
    },
    subscribe(cb: () => void): UnlistenFn {
      subs.add(cb);
      return () => {
        subs.delete(cb);
      };
    },
    setLoggedIn,
    refresh,
    createFav,
    renameFav,
    deleteFav,
    addMembers: addMembersOp,
    removeMember: removeMemberOp,
  };
}

/// useFavorites 返回面(T8 起 UI 消费;成员签名与计划书钉死一致)。
export interface FavStoreApi {
  state: FavStoreState;
  refresh(): Promise<void>;
  createFav(name: string): Promise<boolean>;
  renameFav(id: string, name: string): Promise<boolean>;
  deleteFav(id: string): Promise<boolean>;
  addMembers(id: string, members: FavMember[]): Promise<number>;
  removeMember(id: string, member: FavMember): Promise<boolean>;
}

/// 常用目录 store hook:每组件实例一引擎,状态经订阅镜像进 React
/// (useState),登录态翻转/卸载经 effect 驱动 setLoggedIn(清理即登出
/// 复位,解订 data_changed)。引擎方法为稳定闭包,可直接透传。
export function useFavorites(loggedIn: boolean): FavStoreApi {
  const core = useMemo(() => createFavoritesCore(), []);
  const [state, setState] = useState<FavStoreState>(core.state);

  // 先挂订阅再翻登录态(声明序):首拉结果不会漏镜像。
  // 回调现场读快照(setState 必须显式入参,不能直接当零参回调)。
  useEffect(() => core.subscribe(() => setState(core.state)), [core]);
  useEffect(() => {
    core.setLoggedIn(loggedIn);
    return () => core.setLoggedIn(false);
  }, [core, loggedIn]);

  return {
    state,
    refresh: core.refresh,
    createFav: core.createFav,
    renameFav: core.renameFav,
    deleteFav: core.deleteFav,
    addMembers: core.addMembers,
    removeMember: core.removeMember,
  };
}
