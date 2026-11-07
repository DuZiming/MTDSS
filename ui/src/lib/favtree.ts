/// 侧栏快捷树「成员内容合并」纯投影层(V3-3 返工二 §14.1,2026-11-06 钦定):
/// 展开快捷目录条目 → 各在线成员目录**内容的并集**(文件与文件夹混排、
/// 行带来源终端色点)为其树子级,推翻 §13.1 的成员目录节点形态。本模块
/// 只做投影,不发请求:拉取引擎复用 lib/favfetch 的 useFavListing(Sidebar
/// 每个展开的快捷一个实例,收起即卸载 = 重开即新世代;devices 快照变化
/// 由引擎软更新翻桶),本模块消费其 MemberState 快照。
///
/// 排序裁定:侧栏树无排序状态,固定「目录恒前 + 名称升序」(与主视图
/// 同一 sortEntries 管线,zhCmp 注入;sortEntries 稳定,主比并列保持
/// states 序 = 成员添加序,先并入成员在前)。同路径跨源 = 两行并存、
/// 不合并(行 key=`${device}\u0000${entry.path}`,与 mixedview.MixedRow
/// 同构,NUL 分隔无歧义);来源色点由渲染层 colors.get(row.device) 取。
///
/// 离线/失败成员条目缺席(§14.1:离线成员的条目缺席);absentMemberCount
/// = offline + failed(both 条目永不到达,合并层尾部注记行「N 个成员
/// 离线」的计数来源;ok / pending 不计)。全部纯函数一律返回新对象,
/// 不改入参。

import { type SortState } from "./browse";
import type { Cmp } from "./collate";
import { mixedRows, unavailableCount, type MemberState, type MixedRow } from "./mixedview";

/// 侧栏树固定排序:目录恒前 + 名称升序(主视图 SortState 同形,SortState
/// 管线同一判定核心;注入 cmp 由调用方定,zhCmp())。
export const FAV_TREE_SORT: SortState = { key: "name", dir: "asc" };

/// 成员内容合并行:各 ok 成员条目平铺(成员添加序)→ sortEntries 统一重排
/// (目录恒前 + 名称升序;跨源按名交织,同名并列保持成员添加序)。任意
/// 到达时刻调用输出均正确(渐进渲染,mixedview 同裁定)。
export function mergeMemberListings(states: MemberState[], cmp: Cmp): MixedRow[] {
  return mixedRows(states, FAV_TREE_SORT, cmp);
}

/// 条目缺席成员计数 = offline + failed(注记行「N 个成员离线」;语义上
/// failed 成员条目同样缺席,并入同一注记,不单开错误行——混合视图状态栏
/// unavailableCount 同款口径)。
export function absentMemberCount(states: MemberState[]): number {
  return unavailableCount(states);
}
