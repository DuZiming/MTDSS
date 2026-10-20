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

/// 导航位置:设备列表,或某设备下的一个目录。
export type Loc = { kind: "devices" } | { kind: "dir"; device: string; path: string };

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
function splitPath(p: string): { sep: "\\" | "/"; segs: string[] } {
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
/// 是设备层(就地,便于调用方无条件回退)。
export function parentLoc(loc: Loc): Loc {
  if (loc.kind === "devices") return loc;
  const pp = parentPath(loc.path);
  if (pp == null) return { kind: "devices" };
  return { kind: "dir", device: loc.device, path: pp };
}
