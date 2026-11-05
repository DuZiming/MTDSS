/// 常用目录混合视图纯模型层:多成员目录列表的渐进合并、分组节、离线/失败
/// 桶与跨源多选分组(V3-3 favorites spec §5.2,T9/T10/T11/T12 消费)。
///
/// 渐进渲染裁定(spec §5.2):对每个成员并行 list,**应答逐成员到达、逐次
/// 过合并-排序管线**(行渐进出现,不等齐)。因此 mixedRows / groupedSections
/// 在任意到达时刻的输出都必须正确:每次调用按 states 现状全量重排(排序
/// 复用 browse.sortEntries,目录恒前);sortEntries 稳定,主比并列保持
/// states 序 = 成员添加序(先并入的成员在前)。
///
/// 同路径跨源 = 两行并存、不合并(spec §5.2 同名靠色点区分):行
/// key=`${device}\u0000${entry.path}`,NUL 分隔与 favorites.memberKey 同构,
/// 拼接无歧义,同 path 跨源两行 key 必不同。
///
/// 离线/失败分桶:offline = 设备快照判定(本模块只按 online 回调分桶,
/// 发不发 list 由 T9 调用方决定);failed = list 失败/超时后入桶;
/// pending = 在途。unavailableCount = offline + failed(状态栏「N 个成员
/// 离线或不可用」)。全部纯函数一律返回新对象,不改入参。

import { sortEntries, type FsEntry, type SortState } from "./browse";
import type { Cmp } from "./collate";
import { memberKey, type FavMember } from "./favorites";

/// 混合视图单行:条目 + 来源设备 + 行键(device 与 entry.path NUL 拼接,
/// 同 path 跨源两行 key 必不同——spec §5.2 同名并存)。
export interface MixedRow {
  entry: FsEntry;
  device: string;
  key: string;
}

/// 成员拉取状态:pending=在途;ok=已并入;offline=快照判定离线;
/// failed=list 失败/超时。
export type MemberStatus = "pending" | "ok" | "offline" | "failed";

/// 单成员状态:entries 仅 ok 态承载列表结果(其余态调用方置 [])。
export interface MemberState {
  member: FavMember;
  status: MemberStatus;
  entries: FsEntry[];
}

/// 快照分桶(states 初值):按 online 回调把每个成员判为 pending(在途,
/// list 由调用方发)或 offline(立即入不可用桶,不发 list);entries 恒空;
/// 返回序 = 成员添加序。
export function offlineMembers(members: FavMember[], online: (d: string) => boolean): MemberState[] {
  return members.map((member) => ({ member, status: online(member.device) ? "pending" : "offline", entries: [] }));
}

/// 成员状态上移:按 memberKey 匹配——已在 states 中 → 原位替换(保成员
/// 添加序);不在 → 尾部追加(新添加序)。entries 原样落(ok=列表结果,
/// failed/offline 由调用方置 [])。纯函数返回新数组,不改入参。
export function memberStateUpsert(
  states: MemberState[],
  member: FavMember,
  status: MemberStatus,
  entries: FsEntry[],
): MemberState[] {
  const k = memberKey(member);
  const idx = states.findIndex((s) => memberKey(s.member) === k);
  const next: MemberState = { member, status, entries };
  if (idx < 0) return [...states, next];
  const out = states.slice();
  out[idx] = next;
  return out;
}

/// 行拼装:按 states 序只取 ok 成员 → MixedRow[](平铺序 = 成员添加序)。
function buildRows(states: MemberState[]): MixedRow[] {
  const rows: MixedRow[] = [];
  for (const st of states) {
    if (st.status !== "ok") continue;
    for (const entry of st.entries) {
      rows.push({ entry, device: st.member.device, key: `${st.member.device}\u0000${entry.path}` });
    }
  }
  return rows;
}

/// 行排序:抽 FsEntry 过 sortEntries 统一排(目录恒前),再按条目引用映射
/// 回行。sortEntries 只重排不克隆条目,以 Map 引用回查;同一条目对象出现
/// 两次(引用重复)时按原相对次序逐个出队,与稳定排序并列序一致。
function sortRows(rows: MixedRow[], sort: SortState, cmp: Cmp): MixedRow[] {
  const queue = new Map<FsEntry, MixedRow[]>();
  for (const r of rows) {
    const q = queue.get(r.entry);
    if (q) q.push(r);
    else queue.set(r.entry, [r]);
  }
  return sortEntries(
    rows.map((r) => r.entry),
    sort,
    cmp,
  ).map((entry) => queue.get(entry)!.shift()!);
}

/// 混合模式全列表:各 ok 成员行平铺(成员添加序)→ sortEntries 统一重排
/// (主比并列稳定 = 先并入成员在前,spec §5.2 渐进渲染裁定)→ 映射回
/// MixedRow。任意到达时刻调用输出均正确。
export function mixedRows(states: MemberState[], sort: SortState, cmp: Cmp): MixedRow[] {
  return sortRows(buildRows(states), sort, cmp);
}

/// 分组模式节:节序 = states 序(成员添加序);离线/失败/在途节 rows=空
/// (无条目);ok 节内行 = 该成员条目按当前排序(目录恒前)。
export interface GroupedSection {
  member: FavMember;
  state: MemberState;
  rows: MixedRow[];
}

export function groupedSections(states: MemberState[], sort: SortState, cmp: Cmp): GroupedSection[] {
  return states.map((state) => ({ member: state.member, state, rows: mixedRows([state], sort, cmp) }));
}

/// 不可用成员计数 = offline + failed(ok / pending 不计)。
export function unavailableCount(states: MemberState[]): number {
  return states.filter((s) => s.status === "offline" || s.status === "failed").length;
}

/// 跨源分组结果:一个来源设备的选中条目(组内保持渲染序)。
export interface SourceSel {
  device: string;
  entries: FsEntry[];
}

/// 多选跨源分组(多选拖拽复制到本地 = 按来源设备分组发 N 个传输,
/// spec §5.2):selected 为行键(MixedRow.key)集;组序 = 设备在 rows
/// (渲染序)中首次出现序,组内保持渲染序;未选中行不入。
export function groupRowsByDevice(rows: MixedRow[], selected: ReadonlySet<string>): SourceSel[] {
  const byDevice = new Map<string, FsEntry[]>();
  for (const r of rows) {
    if (!selected.has(r.key)) continue;
    const bucket = byDevice.get(r.device);
    if (bucket) bucket.push(r.entry);
    else byDevice.set(r.device, [r.entry]);
  }
  return [...byDevice].map(([device, entries]) => ({ device, entries }));
}

/// 分组下载推进(T11 评审 Important #1 修复):逐组发起,任一组挂起冲突
/// (isParked true——调用方读 conflictParkedRef 现值,异步循环闭包内
/// state 恒旧,必须走 ref)即停。挂起组待用户裁决后自行传输,其后各组
/// **不再发起**:冲突对话框是单一槽位,继续跑会让后组冲突批整体顶掉
/// 前组待决批,前组既不传输不报错(静默丢组)——停 + 计数提示把它变
/// 可见。纯控制流不触碰 IO,测试可直驱;返回 initiated = 已直发组数、
/// rest = 未发起组数(挂起组自身不计,末组挂起 / 全发完时 rest=0)。
export async function runGroupsWithConflictPause<T>(
  groups: readonly T[],
  run: (g: T) => Promise<void>,
  isParked: () => boolean,
): Promise<{ initiated: number; rest: number }> {
  let initiated = 0;
  for (const g of groups) {
    await run(g);
    if (isParked()) break;
    initiated += 1;
  }
  return { initiated, rest: groups.length - initiated - (initiated < groups.length ? 1 : 0) };
}
