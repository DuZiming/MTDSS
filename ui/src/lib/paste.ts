/// 同设备 copy/move 的冲突分派(Task 4 纯逻辑面;M4b 移交债①扩 local_copy)。
///
/// 粘贴/拖拽分派出同设备面(device_copy 或本机 local_copy)后,执行前须
/// 与目标目录做同名预扫并交用户裁决(与跨设备传输面同一套 scanConflicts +
/// ConflictDialog 四键流程):本模块把「预扫 + op 参数装配 + 冲突策略键
/// 换算」收成纯函数 [`planSameDeviceOp`],App 层注入真正的发送闭包(send
/// → device_copy 走 daemon ctrl copy/move RPC,local_copy 走 transfer.
/// start 作业面);本模块不触碰 daemon/桥。
///
/// 判定键契约(M4a-3 定稿,Task 3 落地):同机 copy 的策略匹配键 = 引擎
/// `policy_key(目标目录, 条目名)`——目录含 `\` 用 `\`,否则 `/`,目录自
/// 带尾分隔符不重复(与 [`joinDirName`] 同规则,与 CLI `join_remote` 零
/// 漂移);move = 发起端**原样 `to` 串**。对话框/决策表的 rel 是裸条目名
/// (scanConflicts 语义),exec 落定时按上述契约换算成判定键再交 send——
/// move 的 pairs `to` 与判定键同规则拼出,天然一致。local_copy 的
/// skip/overwrite 与 ctrl 面同一判定键口径(core fs `policy_key`/
/// move 的 to 串),故换算逻辑原样复用。

import type { FsEntry } from "./browse";
import { baseName, joinDirName } from "./clipboard";
import type { CtrlPolicy, TransferStartParams } from "./daemon";
import { scanConflicts, type Conflict } from "./conflict";

/// op 参数材料(daemon ctrl 面的入参形):copy → paths+dir;move →
/// rename 对(to = 目标目录拼源路径末段)。
export type SameDeviceArgs =
  | { op: "copy"; paths: string[]; dir: string }
  | { op: "move"; pairs: { from: string; to: string }[] };

/// 冲突裁决落定后的发送面(App 层注入:收 op 材料与已换算策略表,真正
/// 发 RPC 并收尾刷新)。纯函数面不做 IO。
export type SameDeviceSender = (args: SameDeviceArgs, policy: CtrlPolicy) => void;

/// 同设备分派计划:预扫冲突行(空 = 无冲突可直发)+ 冲突裁决后的执行
/// 闭包(policy 允许空表 = 无冲突直发/全 legacy;rel → 判定键换算在本
/// 模块收口,调用方拿到的策略表已是引擎判定键)。
export interface SameDevicePlan {
  conflicts: Conflict[];
  exec: (policy: CtrlPolicy) => void;
}

/// 同设备 copy/move 分派(纯函数):op 定执行形态(copy = ctrl copy 自动
/// 改名面;move = ctrl move rename 对),sources 对目标目录列表按
/// scanConflicts 语义预扫(文件对文件/类型不匹配 → 冲突,目录对目录恒
/// 合并,无同名 → 无冲突)。send 由调用方注入,exec(policy) 时以装配好
/// 的 args + 换算好判定键的 policy 调用一次;不改入参。
export function planSameDeviceOp(
  op: "copy" | "move",
  sources: readonly FsEntry[],
  targetDir: string,
  dstEntries: readonly FsEntry[],
  send: SameDeviceSender,
): SameDevicePlan {
  const conflicts = scanConflicts(sources, dstEntries);
  const args: SameDeviceArgs =
    op === "copy"
      ? { op: "copy", paths: sources.map((s) => s.path), dir: targetDir }
      : {
          op: "move",
          pairs: sources.map((s) => ({
            from: s.path,
            to: joinDirName(targetDir, baseName(s.path)),
          })),
        };
  const toKeys = (rels: readonly string[]): string[] =>
    rels.map((r) => joinDirName(targetDir, r));
  return {
    conflicts,
    exec: (policy) =>
      send(args, { skip: toKeys(policy.skip), overwrite: toKeys(policy.overwrite) }),
  };
}

/// local_copy 作业的 transfer.start 参数装配(M4b 移交债①,纯函数):
/// 计划材料(paths/dir/cut)+ 冲突裁决后的策略表(**已经
/// [`planSameDeviceOp`] 换算为引擎判定键**)→ daemon transfer.start
/// local_copy 形态,与 api.rs 契约逐字对齐。skip/overwrite 恒在场(空表
/// = 无冲突直发/legacy 自动改名,与其余 transfer 形态同口径);cut=true
/// = 本机移动快路(daemon 侧自源末段拼 to 对,判定键 = to 串,与本模块
/// move 换算零漂移)。
export function localCopyStartParams(
  plan: { paths: string[]; dir: string; cut: boolean },
  policy: CtrlPolicy,
): TransferStartParams {
  return {
    kind: "local_copy",
    paths: plan.paths,
    dir: plan.dir,
    skip: policy.skip,
    overwrite: policy.overwrite,
    cut: plan.cut,
  };
}
