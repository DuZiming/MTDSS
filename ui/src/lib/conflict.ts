/// 冲突预扫与冲突对话框的纯逻辑面(Task 12)。
///
/// 预扫语义与 daemon `ops::scan_conflicts` 逐字镜像(操作端冲突决策的
/// 数据面,M3c 定稿口径):比对按**名字相等**——
/// - 同名双双为**文件** → 冲突(同名内容之争,须用户裁决);
/// - 同名**类型不匹配**(文件↔目录,双向)→ 冲突(引擎 overwrite 只清
///   文件型占用,目录占用走自动改名安全网,如实报冲突交上层裁决);
/// - 同名**目录对目录** → 恒合并(create_dir_all 语义),不进对话框;
/// - 无同名 → 无冲突。
///
/// 预扫为**顶层粒度**(V1 已知限制,与 CLI 同口径):嵌套同名交给引擎
/// 自动改名 `name (1)` 安全网。目标侧粒度 = 目标目录的一层列表(list
/// RPC);本机目标(下载/本机复制)同样经 list(self 短路直调本地 fs)。
///
/// 对话框四键(spec §4.2):跳过 / 覆盖,批量时加 全部跳过 / 全部覆盖;
/// **全部键只作用剩余未决项**(已决项保持用户先前的选择)。

import type { FsEntry } from "./browse";

/// 预扫源侧条目的最小投影(名字 + 是否目录 + 大小;目录大小对冲突判定
/// 无意义,恒 0)。
export interface ConflictSource {
  name: string;
  is_dir: boolean;
  size: number;
}

/// 预扫出的冲突行(镜像 daemon `ops::Conflict` 字段面,另携 dst_is_dir
/// 供类型不匹配行的展示):UI 问询行的数据源,双列呈现源大小 vs 目标
/// 大小/修改时间。
export interface Conflict {
  /// 源侧名(顶层粒度即源条目名)。
  rel: string;
  /// 源侧是否目录(类型不匹配展示用)。
  src_is_dir: boolean;
  /// 源侧字节数(目录为 0)。
  src_size: number;
  /// 目标侧是否目录(daemon 侧 Conflict 不携带;UI 展示类型不匹配行用)。
  dst_is_dir: boolean;
  /// 目标侧字节数(目录为 0)。
  dst_size: number;
  /// 目标侧修改时间(毫秒;问询行「目标修改时间」列)。
  dst_mtime_ms: number;
}

/// 冲突预扫(纯函数):源条目表对目标目录列表按名字比对,分类规则见
/// 模块文档。输出按源序稳定;不改入参。
export function scanConflicts(
  sources: readonly ConflictSource[],
  dstEntries: readonly FsEntry[],
): Conflict[] {
  return sources.flatMap((s) => {
    const d = dstEntries.find((e) => e.name === s.name);
    if (d == null) return [];
    // 文件对文件 → 冲突;类型不匹配 → 冲突;目录对目录 → 恒合并(无)。
    if (!(s.is_dir !== d.is_dir || (!s.is_dir && !d.is_dir))) return [];
    return [
      {
        rel: s.name,
        src_is_dir: s.is_dir,
        src_size: s.size,
        dst_is_dir: d.is_dir,
        dst_size: d.size,
        dst_mtime_ms: d.mtime_ms,
      },
    ];
  });
}

/// 单条冲突的用户裁决。
export type ConflictDecision = "skip" | "overwrite";

/// 决策表:rel → 裁决(未入表 = 未决)。
export type DecisionMap = Readonly<Record<string, ConflictDecision>>;

/// 对话框分组(纯函数):1 条 → 单条形态(仅 跳过/覆盖 两键);≥2 条 →
/// 批量形态(加 全部跳过/全部覆盖 两键)。rows 原样保序返回(同一份
/// 引用,渲染层免拷贝)。
export function groupConflicts(conflicts: readonly Conflict[]): {
  single: boolean;
  rows: readonly Conflict[];
} {
  return { single: conflicts.length === 1, rows: conflicts };
}

/// 「全部跳过/全部覆盖」落定(纯函数):**只作用剩余未决项**——已决项
/// 保持原裁决不被全部键翻案;返回新对象,不改入参。
export function applyAllDecisions(
  decisions: DecisionMap,
  conflicts: readonly Conflict[],
  all: ConflictDecision,
): Record<string, ConflictDecision> {
  const out: Record<string, ConflictDecision> = { ...decisions };
  for (const c of conflicts) {
    if (out[c.rel] == null) out[c.rel] = all;
  }
  return out;
}

/// 决策表 → 传输策略名单(纯函数):已决项按决投影进 skip/overwrite;
/// 未决项不进任何名单(调用方门控「全部决完才放行」,未决残留即不得
/// 发起传输——直接放行会让未决冲突走引擎自动改名安全网,违背对话框
/// 的问询语义)。
export function policyFromDecisions(
  conflicts: readonly Conflict[],
  decisions: DecisionMap,
): { skip: string[]; overwrite: string[] } {
  const skip: string[] = [];
  const overwrite: string[] = [];
  for (const c of conflicts) {
    const d = decisions[c.rel];
    if (d === "skip") skip.push(c.rel);
    else if (d === "overwrite") overwrite.push(c.rel);
  }
  return { skip, overwrite };
}

/// 剩余未决项计数(对话框「全部跳过(剩余 N)」按钮标签用)。
export function undecidedCount(
  conflicts: readonly Conflict[],
  decisions: DecisionMap,
): number {
  return conflicts.filter((c) => decisions[c.rel] == null).length;
}
