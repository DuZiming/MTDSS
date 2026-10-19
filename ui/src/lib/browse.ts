/// 远端文件浏览的展示逻辑层:列表切片(排序/过滤)、大小与时间格式化、
/// 进入目录门控、列表响应的 seq 守卫。全部纯函数,Task 7 组件层消费。

import type { Cmp } from "./collate";

/// `list` RPC 结果中的单个条目(字段名与 Rust 侧序列化逐字对齐)。
export interface FsEntry {
  name: string;
  path: string;
  size: number;
  mtime_ms: number;
  is_dir: boolean;
}

/// `list` RPC 结果切片(entries 可能因条目上限截断)。
export interface Listing {
  entries: FsEntry[];
  truncated: boolean;
}

/// 条目排序:目录恒在最前(升/降序皆然);组内按名称 cmp,desc 仅组内反序。
/// 返回新数组不改入参;同名条目保持原始相对次序(sort 稳定)。
export function sortEntries(es: FsEntry[], dir: "asc" | "desc", cmp: Cmp): FsEntry[] {
  const out = [...es];
  out.sort((a, b) => {
    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
    const c = cmp(a.name, b.name);
    return dir === "asc" ? c : -c;
  });
  return out;
}

/// 名称过滤:大小写不敏感子串包含;q 为空 → 原样内容(新数组)。
export function filterEntries(es: FsEntry[], q: string): FsEntry[] {
  const needle = q.toLowerCase();
  if (needle === "") return [...es];
  return es.filter((e) => e.name.toLowerCase().includes(needle));
}

/// 大小格式化:目录 → ""(调用方以 isDir 决定);0 → "0 B";其余 1024 进制,
/// 单位按阈值递进,一位小数且不剪尾("2.0 MB");字节段保持整数;超出表尾
/// 单位(PB)封顶不再递进。
export function fmtSize(n: number, isDir: boolean): string {
  if (isDir) return "";
  if (n === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  let value = n;
  let i = 0;
  while (i < units.length - 1 && value >= 1024) {
    value /= 1024;
    i += 1;
  }
  if (i === 0) return `${value} B`;
  return `${value.toFixed(1)} ${units[i]}`;
}

/// 修改时间格式化:0 → "";否则本地时区 `YYYY/M/D H:MM`(月/日/时不补零,
/// 分钟固定两位),如 2026/10/18 9:05。
export function fmtMtime(ms: number): string {
  if (ms === 0) return "";
  const t = new Date(ms);
  return (
    `${t.getFullYear()}/${t.getMonth() + 1}/${t.getDate()}` +
    ` ${t.getHours()}:${String(t.getMinutes()).padStart(2, "0")}`
  );
}

/// 进入目录门控:目标设备离线,或当前正在浏览离线设备的目录,一律拒绝。
export function canEnterDir(targetOnline: boolean, browsingOffline: boolean): boolean {
  return targetOnline && !browsingOffline;
}

/// 列表响应 seq 守卫:响应序号旧于最新发出序号即过期 → null(丢弃)。
/// 最新(或更新)序号透传原结果;listing 为 null(请求失败)原样透传。
export function applyListing(seq: number, latestSeq: number, listing: Listing | null): Listing | null {
  if (seq < latestSeq) return null;
  return listing;
}
