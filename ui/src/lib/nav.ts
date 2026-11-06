/// 导航位置与历史栈:浏览「后退/前进」的纯函数模型(Task 7 状态层消费)。
///
/// 分隔符告诫:DirEntry.path 由 daemon 按目标平台的原生分隔符下发——
/// Windows 为反斜杠、POSIX 为正斜杠,且同一批条目可能来自不同平台的设备。
/// UI 层严禁以 '/' 单独切分或以 '\' 拼装路径;凡需分解/取父/拼段,一律
/// 经由本模块(splitPath/parentPath/pathSegments/locAtSegment,按是否含
/// '\' 显式探测分隔符),新代码不得另写路径解析。
///
/// 边界语义:Windows 反斜杠路径以盘符为第一段("C:\a\b" → ["C:","a","b"]),
/// POSIX 以 "/" 为第一段("/a/b" → ["/","a","b"]);盘根("C:\")与 POSIX 根
/// ("/")的上级位置是设备层(devices)。仅考虑绝对路径(daemon 下发的
/// Rust 侧路径恒为绝对);尾分隔符与无尾分隔等价("C:\a\" ≡ "C:\a")。

/// 导航位置:设备列表、某设备下的一个目录、常用目录视图位,或常用内钻取
/// 的快捷命名空间位(V3-3 返工 §13.6:base=成员路径,rel=相对子路径,
/// rel="" = 成员根;设备名与原始路径在面包屑层隐藏,仅此 Loc 持有)。
export type Loc =
  | { kind: "devices" }
  | { kind: "dir"; device: string; path: string }
  | { kind: "fav"; id: string }
  | { kind: "favdir"; favId: string; device: string; base: string; rel: string };

/// 导航历史:back 栈(旧 → 新)、fwd 栈(新 → 旧)、当前位。
export interface NavState {
  back: Loc[];
  fwd: Loc[];
  current: Loc;
}

/// 前进:current 压入 back、fwd 清空、current 换为 next。
/// 重复点击同一位仍 push(调用方以重复导航表达刷新语义,历史保真)。
export function navGo(h: NavState, next: Loc): NavState {
  return { back: [...h.back, h.current], fwd: [], current: next };
}

/// 后退:back 栈顶弹出为 current,current 移入 fwd;back 为空返回 null。
export function navBack(h: NavState): NavState | null {
  if (h.back.length === 0) return null;
  return {
    back: h.back.slice(0, -1),
    fwd: [h.current, ...h.fwd],
    current: h.back[h.back.length - 1],
  };
}

/// 前进:fwd 栈顶弹出为 current,current 压入 back;fwd 为空返回 null。
export function navForward(h: NavState): NavState | null {
  if (h.fwd.length === 0) return null;
  return {
    back: [...h.back, h.current],
    fwd: h.fwd.slice(1),
    current: h.fwd[0],
  };
}

/// 分解路径为段并剥去尾部分隔符产生的空段("C:\a\"、"/a/" 与无尾形式等价)。
/// P4b Task 4 起导出:lib/address.ts 编辑态解析剥余段尾分隔符复用(模块
/// 文档纪律:路径分解一律经本模块,新代码不另写解析)。
export function splitPath(p: string): { sep: "\\" | "/"; segs: string[] } {
  const sep: "\\" | "/" = p.includes("\\") ? "\\" : "/";
  const segs = p.split(sep);
  while (segs.length > 1 && segs[segs.length - 1] === "") segs.pop();
  return { sep, segs };
}

/// 父目录路径:已到根(盘根 / POSIX 根 / 单段相对名)返回 null。
export function parentPath(p: string): string | null {
  const { sep, segs } = splitPath(p);
  if (segs.length <= 1) return null;
  // Windows 恰两段("C:\a")父为盘根,规范形带尾分隔符("C:\")。
  if (sep === "\\" && segs.length === 2) return segs[0] + "\\";
  const parent = segs.slice(0, -1).join(sep);
  // POSIX 绝对路径剥到只剩前导空段时,父即根 "/"。
  if (parent === "" && p.startsWith(sep)) return sep;
  return parent;
}

/// 路径段列表(面包屑数据源):盘符与 POSIX 根各占一段。
export function pathSegments(p: string): string[] {
  if (p === "") return [];
  const { sep, segs } = splitPath(p);
  // POSIX 绝对路径:前导空段呈现为独立的 "/" 段。
  if (sep === "/" && segs.length > 0 && segs[0] === "") return ["/", ...segs.slice(1)];
  return segs;
}

/// 面包屑第 k 段(1 起)对应的导航位;k <= 0 为设备层;k 超出段数截到最深。
export function locAtSegment(dev: string, p: string, segCount: number): Loc {
  if (segCount <= 0) return { kind: "devices" };
  const { sep, segs } = splitPath(p);
  const k = Math.min(segCount, segs.length);
  if (sep === "/") {
    // POSIX:k=1 即根 "/";前导空段不重复计入。
    if (segs.length > 0 && segs[0] === "") {
      return { kind: "dir", device: dev, path: "/" + segs.slice(1, k).join("/") };
    }
    return { kind: "dir", device: dev, path: segs.slice(0, k).join("/") };
  }
  // Windows:k=1 即盘根("C:\"),k>=2 为 "C:\a" 形。
  const path = k === 1 ? segs[0] + "\\" : segs.slice(0, k).join("\\");
  return { kind: "dir", device: dev, path };
}

/// 上级位置:目录位逐级上溯,到盘根/POSIX 根后即设备层;设备层的上级仍
/// 是设备层(就地,便于调用方无条件回退);常用视图上级=设备层
/// (V3-3 裁量 #5);快捷命名空间位(V3-3 返工 §13.6)在虚拟路径内逐级
/// 上溯,rel="" 顶级回该常用混合视图(fav),绝不露设备层/原始路径。
export function parentLoc(loc: Loc): Loc {
  if (loc.kind === "fav") return { kind: "devices" }; // 常用视图上级=设备层(V3-3 裁量 #5)
  if (loc.kind === "favdir") {
    // 顶级(rel="")回该常用混合视图;否则 rel 去一段(mirror parentPath
    // 纪律:splitPath 显式探测分隔符),单段 rel 去后即成员根(rel="")。
    if (loc.rel === "") return { kind: "fav", id: loc.favId };
    const { sep, segs } = splitPath(loc.rel);
    return {
      kind: "favdir",
      favId: loc.favId,
      device: loc.device,
      base: loc.base,
      rel: segs.length <= 1 ? "" : segs.slice(0, -1).join(sep),
    };
  }
  if (loc.kind === "devices") return loc;
  const pp = parentPath(loc.path);
  if (pp == null) return { kind: "devices" };
  return { kind: "dir", device: loc.device, path: pp };
}

/// 快捷命名空间拼段(V3-3 返工 §13.6):成员路径 base(平台原生分隔符)
/// 接相对子路径 rel(""\= 成员根本身,即 base 原串)。分隔符取 base 经
/// splitPath 显式探测(nav 纪律:勿手工拼装);base 带尾分隔符(盘根/
/// POSIX 根形)直接接 rel,否则补一个分隔符。
export function joinRel(base: string, rel: string): string {
  if (rel === "") return base;
  const { sep } = splitPath(base);
  return base.endsWith(sep) ? base + rel : base + sep + rel;
}

/// 绝对子路径反推快捷命名空间 rel(V3-3 返工 §13.6 钻取数据源):child 即
/// base → "";child 在 base 之下 → 剥前缀余段;否则 null(child 不在该成员
/// 之下,调用方防御回退普通目录导航)。分隔符随 base。
export function relUnderBase(base: string, child: string): string | null {
  if (child === base) return "";
  const { sep } = splitPath(base);
  const prefix = base.endsWith(sep) ? base : base + sep;
  return child.startsWith(prefix) ? child.slice(prefix.length) : null;
}
