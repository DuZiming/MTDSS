/// 常用目录(账号级 KV 文档)schema 与终端标志色板:解析/编辑/去重纯函数
/// + 设备位次取色(spec 2026-11-05-mtdss-v3p3-favorites §3 数据模型与同步 /
/// §13.4 返工钦定 64 色板:用户逐格挑选,按终端加入账号顺序分配)。kv 键
/// `favorites`,整单 LWW 读写,编辑由调用方 store「kv.get 取最新 → 本地改
/// → kv.put 整单回写」驱动。
///
/// 前向守卫(spec §3.2):解析出 v > 1 → readOnly=true,调用方不渲染编辑
/// 入口且绝不回写(防旧版 schema 覆盖新版数据);结构已知字段尽力解析。
/// 解码失败且无 v 可读 → readOnly=true + 空文档(EMPTY_FAVS)。
/// 容忍字段缺失:updated_at→0、favorites→[]、created_at→0、members→[]。
///
/// 成员只存 device_id + path 原串,设备名/平台/标志色渲染时从设备快照 join,
/// 不入库;常用条目序=创建序(尾部追加),成员序=添加序(尾部追加),均不做
/// 手动排序。本模块全部纯函数一律返回新对象,不改入参;updated_at 由调用方
/// (store)写回前统一更新,编辑函数不改时间字段(createFavorite 除外,其
/// now 参数即新条目时间戳)。

import type { FsEntry } from "./browse";
import { relUnderBase } from "./nav";

/// account_kv 键名(spec §3.1,ASCII 定死)。
export const FAVORITES_KV_KEY = "favorites";

/// 常用成员:设备 id + 目录路径原串(路径分隔符随目标平台,勿在本模块切分)。
export interface FavMember {
  device: string;
  path: string;
}

/// 单个常用条目:id=客户端生成 16 hex;空成员(0 个)合法。
export interface Favorite {
  id: string;
  name: string;
  created_at: number;
  members: FavMember[];
}

/// 整单文档(v 恒为 1;更高 v 见 parseFavorites 只读守卫)。
export interface FavoritesDoc {
  v: 1;
  updated_at: number;
  favorites: Favorite[];
}

/// 空文档:kv 无值时的合法初始态(404=空清单,spec §3.2)。
export const EMPTY_FAVS: FavoritesDoc = { v: 1, updated_at: 0, favorites: [] };

/// 解析结果:doc 为规范化文档,readOnly=true 时调用方禁编辑、绝不回写。
export interface FavsParse {
  doc: FavoritesDoc;
  readOnly: boolean;
}

/// 解析 kv 存值 → 规范化文档 + 只读旗标(spec §3.1/§3.2 前向守卫):
/// null/空串 → null(无键);v=1 → 可编辑;v>1 → 只读 + 已知字段尽力解析;
/// 解码失败 / 非对象 / 无 v 可读 → 只读 + 空文档;字段缺失按容差补默认。
export function parseFavorites(raw: string | null): FavsParse | null {
  if (raw === null || raw === "") return null;
  let obj: unknown;
  try {
    obj = JSON.parse(raw);
  } catch {
    return { doc: EMPTY_FAVS, readOnly: true };
  }
  if (typeof obj !== "object" || obj === null || Array.isArray(obj)) {
    return { doc: EMPTY_FAVS, readOnly: true };
  }
  const rec = obj as Record<string, unknown>;
  if (typeof rec.v !== "number") return { doc: EMPTY_FAVS, readOnly: true };
  const readOnly = rec.v > 1;
  const favorites = Array.isArray(rec.favorites)
    ? rec.favorites.map(parseFavorite).filter((f): f is Favorite => f !== null)
    : [];
  const updated_at = typeof rec.updated_at === "number" ? rec.updated_at : 0;
  return { doc: { v: 1, updated_at, favorites }, readOnly };
}

/// 尽力解析单个常用条目:id/name 非字符串(身份与展示缺失)即丢弃;
/// created_at 缺 → 0;members 缺/含残项 → 逐项过滤(FavMember 双字符串字段)。
function parseFavorite(f: unknown): Favorite | null {
  if (typeof f !== "object" || f === null) return null;
  const rec = f as Record<string, unknown>;
  if (typeof rec.id !== "string" || typeof rec.name !== "string") return null;
  const members = Array.isArray(rec.members)
    ? rec.members
        .map((mem): FavMember | null => {
          if (typeof mem !== "object" || mem === null) return null;
          const mr = mem as Record<string, unknown>;
          return typeof mr.device === "string" && typeof mr.path === "string"
            ? { device: mr.device, path: mr.path }
            : null;
        })
        .filter((mem): mem is FavMember => mem !== null)
    : [];
  const created_at = typeof rec.created_at === "number" ? rec.created_at : 0;
  return { id: rec.id, name: rec.name, created_at, members };
}

/// 生成常用条目 id:crypto.getRandomValues 8 字节 → 16 位小写 hex(spec §3.1)。
export function newFavId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(8));
  let id = "";
  for (const b of bytes) id += b.toString(16).padStart(2, "0");
  return id;
}

/// 新建常用(命名对话框):尾部追加(创建序),空成员合法。
/// created_at 与 updated_at 取 now;id 客户端生成。
export function createFavorite(doc: FavoritesDoc, name: string, now: number): FavoritesDoc {
  return {
    ...doc,
    updated_at: now,
    favorites: [...doc.favorites, { id: newFavId(), name, created_at: now, members: [] }],
  };
}

/// 重命名:只改目标条目名;id 不存在 → 等值返回(新壳同内容)。
export function renameFavorite(doc: FavoritesDoc, id: string, name: string): FavoritesDoc {
  return { ...doc, favorites: doc.favorites.map((f) => (f.id === id ? { ...f, name } : f)) };
}

/// 删除常用=解除链接(spec §3.1,绝不触碰远端文件):只删目标条目。
export function removeFavorite(doc: FavoritesDoc, id: string): FavoritesDoc {
  return { ...doc, favorites: doc.favorites.filter((f) => f.id !== id) };
}

/// 成员去重键:NUL 分隔避免 device/path 拼接歧义(spec §3.1 同 (device,path)
/// 在该常用内去重,跨常用重复允许)。
export function memberKey(m: FavMember): string {
  return `${m.device}\u0000${m.path}`;
}

/// 添加成员:目标常用内按 memberKey 去重(含入参数组内部重复),新成员尾部
/// 追加(添加序);added=实际新增数;目标不存在 → added 0、文档不变。
export function addMembers(doc: FavoritesDoc, id: string, members: FavMember[]): { doc: FavoritesDoc; added: number } {
  let added = 0;
  const favorites = doc.favorites.map((f) => {
    if (f.id !== id) return f;
    const seen = new Set(f.members.map(memberKey));
    const fresh: FavMember[] = [];
    for (const m of members) {
      const k = memberKey(m);
      if (seen.has(k)) continue;
      seen.add(k);
      fresh.push(m);
    }
    added += fresh.length;
    return { ...f, members: [...f.members, ...fresh] };
  });
  return { doc: { ...doc, favorites }, added };
}

/// 移除成员(解除链接):只删目标常用内匹配 memberKey 的那一项。
export function removeMember(doc: FavoritesDoc, id: string, member: FavMember): FavoritesDoc {
  const k = memberKey(member);
  return {
    ...doc,
    favorites: doc.favorites.map((f) => (f.id === id ? { ...f, members: f.members.filter((x) => memberKey(x) !== k) } : f)),
  };
}

/// 「添加到常用目录」候选与门(V3-3 Task 11,目录位右键/多选面):返回
/// 待加入的目录路径集,null = 菜单项不呈现。裁定:右键目标为目录且选择
/// 为空或含该行 → 呈现;多选非空且全为目录 → 呈现;目标行在选择内且选中
/// 全为目录时加整个选中集(资源管理器多选语义),否则只取右键行。设备
/// 离线也可加(路径是字符串,离线面由混合视图承担)。纯函数不改入参。
export function favAddTargets(
  target: FsEntry | null,
  rows: FsEntry[],
  selected: ReadonlySet<string>,
): string[] | null {
  const selRows = rows.filter((r) => selected.has(r.path));
  const allDirs = selRows.length > 0 && selRows.every((r) => r.is_dir);
  if (target != null) {
    if (!target.is_dir) return null;
    if (selected.size === 0 || selected.has(target.path)) {
      return allDirs && selected.has(target.path) ? selRows.map((r) => r.path) : [target.path];
    }
    return null;
  }
  return allDirs ? selRows.map((r) => r.path) : null;
}

/// 常用拖放/粘贴落点解析(V3-3 Task 12,落点键 `fav\u0000<id>` 与 fav 位
/// 粘贴共用):单成员 → 直取该成员(等效既有 device\u0000path 落点,不发弹层);
/// 多成员 → picker(开 FavMemberPicker 选成员);空常用(0 成员)→ null
/// (无落点,调用方静默忽略)。纯函数不改入参。
export type FavDropTarget = { kind: "member"; member: FavMember } | { kind: "picker" };

export function resolveFavDropTarget(fav: Favorite): FavDropTarget | null {
  if (fav.members.length === 1) return { kind: "member", member: fav.members[0] };
  if (fav.members.length > 1) return { kind: "picker" };
  return null;
}

/// 含给定点(device, 绝对路径)的成员中取「最深」者(V3-3 返工 §13.6
/// favdir 钻取的 base 判定:混合视图双击/菜单打开只知条目设备与绝对路径,
/// 同设备多成员嵌套时按最长前缀消歧到最具体成员;child 即成员本身也算包含
/// ——rel="" 语义)。无成员包含 → null(调用方防御回退普通目录导航)。
/// 前缀边界经 nav.relUnderBase(splitPath 显式探测分隔符,纪律:勿手工切分)。
export function memberOfPath(fav: Favorite, device: string, path: string): FavMember | null {
  let best: FavMember | null = null;
  for (const m of fav.members) {
    if (m.device !== device) continue;
    if (relUnderBase(m.path, path) == null) continue;
    if (best == null || m.path.length > best.path.length) best = m;
  }
  return best;
}

/// 终端标志色板(spec §13.4 返工钦定版:用户逐格挑选 64 色 verbatim,8 色
/// 系×8 明暗档行主序矩阵;§4 的 蓝/绿/红/琥珀 状态色回避限制已由用户裁定
/// 作废,含蓝/绿/红/橙)。
export const DEVICE_COLOR_HEX: readonly string[] = [
  "#158cd5", "#d52c15", "#2fd515", "#d58515", "#1548d5", "#d5158f", "#15cfd5", "#7f15d5",
  "#1679b6", "#b62916", "#2cb616", "#b67316", "#1641b6", "#b6167b", "#16b0b6", "#6e16b6",
  "#1e9deb", "#eb361e", "#39eb1e", "#eb961e", "#1e55eb", "#eb1ea0", "#1ee5eb", "#8f1eeb",
  "#1a6593", "#93281a", "#2a931a", "#93611a", "#1a3a93", "#931a67", "#1a8f93", "#5d1a93",
  "#3faef3", "#f3543f", "#57f33f", "#f3a83f", "#3f6ff3", "#f33fb1", "#3fedf3", "#a23ff3",
  "#1f516f", "#6f291f", "#2a6f1f", "#6f4e1f", "#1f356f", "#6f1f52", "#1f6d6f", "#4b1f6f",
  "#1f3e51", "#51251f", "#26511f", "#513c1f", "#1f2d51", "#511f3f", "#1f4f51", "#3b1f51",
  "#1b2c37", "#371e1b", "#1f371b", "#372b1b", "#1b2237", "#371b2c", "#1b3637", "#2a1b37",
];

/// 设备 → 标志色:按终端**加入账号顺序**(created_at 升序)取位次
/// i → 色板[i mod 64];缺 created_at(旧 daemon)排尾,彼此按 device_id,
/// created_at 并列再按 device_id。零服务端状态、三端同算法同结果,≤64 台
/// 必不撞色,>64 台回绕(spec §13.4;服务端 devices 快照本就按 created_at
/// 升序返回,此处重排为双保险,与 Swift FavoritesPalette.colorMap 逐语义
/// 镜像)。返回新 Map,不改入参数组顺序。
export function deviceColorMap(devices: { device_id: string; created_at?: number }[]): Map<string, string> {
  const byId = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);
  const sorted = [...devices].sort((a, b) => {
    if (a.created_at != null && b.created_at != null) {
      return a.created_at !== b.created_at ? a.created_at - b.created_at : byId(a.device_id, b.device_id);
    }
    if (a.created_at == null && b.created_at == null) return byId(a.device_id, b.device_id);
    return a.created_at != null ? -1 : 1;
  });
  const map = new Map<string, string>();
  sorted.forEach((d, i) => map.set(d.device_id, DEVICE_COLOR_HEX[i % DEVICE_COLOR_HEX.length]));
  return map;
}
