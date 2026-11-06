/// 快捷目录混合视图主体(V3-3 Task 9,spec §5.2):多成员目录内容经
/// useFavListing 渐进扇出(应答逐成员并入,mixedRows 全量重排渐进出行,
/// 不等齐),行身份 = 复合键(device\0path,同路径跨源两行并存)、行首
/// 色点 = 来源设备标志色;离线/失败成员不入混排行,在视图底部以
/// 「N 个成员离线或不可用」注记行呈现(title 悬停列设备名+路径明细);
/// 空快捷(0 成员)渲染空态引导。
///
/// 交互裁定:选择集在 App(fav 位 = 复合键集,与 dir 位路径集共用同一
/// selection 槽,导航清空复用 clearTransient);Shift 锚点/范围尾行在本
/// 组件(fav.id 变更由 App 以 key 重挂载复位)。行点击经 applyClickSelect
/// 同款解析(单击/Ctrl/Shift);双击目录行 → onOpenDir(device, path)
/// 钻取普通 dir 位(混合只在第一层,spec §5.2),带多选修饰键不触发
/// (三视图既有守卫),文件双击无操作。行右键上抛 MixedRow(T11 装配,
/// 本任务 App 只存不弹);空白右键透传 App 空白菜单(刷新经 onRetry →
/// reloadTick 重扇出)。Del/Ctrl+X/C/V/工具栏操作在 fav 位为安全 no-op
/// (App 既有处理器有 kind!=="dir" 早退,本组件不改)。
///
/// 三视图复用(T7 可选 rowKey/sourceColor):视图回调只回传 FsEntry/
/// entry.path,以 entry 引用回查行(rowByEntry——MixedRow.entry 是各成员
/// 应答中的原对象,引用唯一);行点击的复合键从事件宿主行渲染的
/// data-row-path 取(同 path 跨源两行据此消歧)。行拖拽 T12 接线
/// (onRowPointerDown 透传 + dragSourcePaths 复合键半透明);cutPaths
/// 视觉仍不启用(裁定记档见 T11)。loading 恒 false(渐进裁定:逐成员
/// 到齐即出行,不整段等待);空态判定按「已有 ok 成员」(全部在途/不可
/// 用时不显示「此文件夹为空」)。
///
/// 分组模式(V3-3 Task 10,viewOrg==="grouped",工具栏「查看·组织」段
/// 切换,全局偏好):groupedSections 按成员分节(节序 = states 序 =
/// 成员添加序),节头行 = chevron + 色点 + 设备名 + 路径末段
/// (nav.pathSegments 分解,勿手工切分)+ 非 ok 态灰标注(离线/无法
/// 访问/加载中…);离线/失败节头灰显(unavailable 类),在线节头宿主
/// 节级拖放落点 device\0path(T12 消费,悬停高亮同 dropHoverKey 比对)。
/// 节头整行点击展开/收起(返工 §13.5:默认全展;收起集 = 会话内 state
/// 不持久,fav.id 换位由 App key 重挂载复位;caret 旋向 = 状态,收起仅
/// 藏节内行,节头/落点/灰标注保留,选择集不修剪——裁定)。节内行与
/// 混合模式同构渲染(复合键选择/色点/右键回调同源,搜索过滤同按行键
/// 集合);空节(ok 无条目)与 offline/failed/pending 节只留节头标注
/// (不渲染空态块)。底部不可用注记两模式一致保留(T9 裁定)。
///
/// 分组+details 单列头(返工 §14.4):该组合**全部节并入一张表**——
/// FileDetails 以 sections(节标签行+节内行交错)单实例渲染,表头全页
/// 恰一枚且粘性,列宽拖动/横向滚动跨节一致(多实例 headerless 方案
/// 弃用,裁定记档见 FileDetails 模块注);节标签行样式/交互/落点契约
/// 原样(节头标记语言由 groupHeadInner 与 list/tiles 逐节分支共用)。
/// list/tiles 无表头概念,仍逐节渲染(原分支不动)。
///
/// 行拖拽(V3-3 Task 12):宿主根 div 挂 onRowPointerDown(App 以同一套
/// 指针拖拽会话机制起步多源分组会话——本组件只透传事件,行命中/选中集
/// 映射/分组全在 App);dragSourcePaths(App 下发的正被拖拽行集,键形 =
/// 复合键 device\0path 与 rowKey 同构)挂 .drag-source 半透明,缺省
/// null(零回归)。宿主根自身即拖放落点 fav\0id;拖出窗口松开 = 解析一
/// 次落点目录后**按来源组 fan-out 落盘**(远端组逐组 download,本机组
/// 跳过;与 dir 位单源 download 机制不同,fan-out 解析在 App)。
import { Fragment, useCallback, useEffect, useMemo, useState } from "react";
import type {
  KeyboardEvent as ReactKeyboardEvent,
  MouseEvent as ReactMouseEvent,
  PointerEvent as ReactPointerEvent,
  ReactNode,
} from "react";
import { useTranslation } from "react-i18next";

import { filterEntries, type FsEntry, type SortKey, type SortState } from "../lib/browse";
import { zhCmp } from "../lib/collate";
import type { DeviceInfo } from "../lib/derived";
import { memberKey, type Favorite } from "../lib/favorites";
import { useFavListing } from "../lib/favfetch";
import {
  groupedSections,
  mixedRows,
  unavailableCount,
  type GroupedSection,
  type MemberStatus,
  type MixedRow,
} from "../lib/mixedview";
import { pathSegments } from "../lib/nav";
import { applyClickSelect, clickMode, type RowClickInfo } from "../lib/select";

import FileDetails, { type DetailsSection } from "./FileDetails";
import FileList from "./FileList";
import FileTiles from "./FileTiles";
import type { DirViewProps } from "./FileList";
import { IcChevronRight } from "./icons";
import type { ViewMode, ViewOrg } from "./Toolbar";

/// fav 位 cut 灰显视觉不启用(T11 裁定记档;行拖拽已 T12 接线):空集常量。
const NO_CUT: ReadonlySet<string> = new Set<string>();

export interface FavViewProps {
  fav: Favorite;
  devices: DeviceInfo[];
  /// 设备 → 标志色(App 派生的 favColors)。
  colors: Map<string, string>;
  viewMode: ViewMode;
  /// 视图组织方式(V3-3 Task 10):混合|分组(全局偏好,App 持有)。
  viewOrg: ViewOrg;
  sortState: SortState;
  /// 刷新世代(App reloadTick;递增即整体重扇出)。
  reloadTick: number;
  /// 复合键(device\0path)选择集(App 持有)。
  selected: ReadonlySet<string>;
  onSelected(next: ReadonlySet<string>): void;
  searchQ: string;
  onOpenDir(device: string, path: string): void;
  onRowContextMenu(row: MixedRow, x: number, y: number): void;
  onBlankContextMenu(x: number, y: number): void;
  onRetry(): void;
  /// details 表头排序键点击(App 以 headerClick 合成,与工具栏同源)。
  onSortKey(key: SortKey): void;
  /// 混合行数上抛(App 状态栏 fav 位 itemCount 消费)。
  onCount(n: number): void;
  /// 渲染行上抛(V3-3 Task 11 动作对等):App 经 ref 持有现值,动作处理器
  /// (下载/删除/复制剪切)调用时做跨源分组;缺省不上抛(零回归)。
  onRowsChange?(rows: MixedRow[]): void;
  /// 行 pointerdown 上抛(V3-3 Task 12 拖拽源):宿主根 div 挂载,App 以
  /// 同一套指针拖拽会话机制起步(行命中/选中集映射/多源分组在 App);
  /// 缺省不挂 = 行不可拖(零回归)。
  onRowPointerDown?(e: ReactPointerEvent): void;
  /// 正被拖拽的行集(V3-3 Task 12;App 下发,键形 = 复合键 device\0path
  /// 与 rowKey 同构):命中行挂 .drag-source 半透明;缺省 null(零回归)。
  dragSourcePaths?: ReadonlySet<string> | null;
  /// 拖拽悬停高亮键(T12 消费;null = 无悬停)。
  dropHoverKey: string | null;
}

export default function FavView({
  fav,
  devices,
  colors,
  viewMode,
  viewOrg,
  sortState,
  reloadTick,
  selected,
  onSelected,
  searchQ,
  onOpenDir,
  onRowContextMenu,
  onBlankContextMenu,
  onRetry,
  onSortKey,
  onCount,
  onRowsChange,
  onRowPointerDown,
  dragSourcePaths,
  dropHoverKey,
}: FavViewProps) {
  const { t } = useTranslation();
  const { states } = useFavListing(fav, devices, reloadTick);
  // Shift 锚点/范围尾行(选择集在 App;fav.id 换位由 App key 重挂载复位)。
  const [anchor, setAnchor] = useState<string | null>(null);
  const [rangeEnd, setRangeEnd] = useState<string | null>(null);

  // 分组节收起集(返工 §13.5):键 = memberKey(device\0path),空集 =
  // 全展(默认),整行点击切换。会话内 state 不持久(裁定);fav.id 换位
  // 由 App key 重挂载复位(与锚点同款),平 useState 即足。
  const [collapsedKeys, setCollapsedKeys] = useState<Set<string>>(new Set());
  const toggleSection = useCallback((key: string) => {
    setCollapsedKeys((s) => {
      const next = new Set(s);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }, []);

  // 渐进合并-排序管线:states 任意到达时刻全量重排(spec §5.2)。
  const allRows = useMemo(() => mixedRows(states, sortState, zhCmp()), [states, sortState]);
  // 搜索过滤:filterEntries 消费 entry.name(与 dir 位同源),引用集合回查行。
  const rows = useMemo(() => {
    if (searchQ === "") return allRows;
    const keep = new Set(filterEntries(allRows.map((r) => r.entry), searchQ));
    return allRows.filter((r) => keep.has(r.entry));
  }, [allRows, searchQ]);
  // 分组模式管线(V3-3 Task 10):节序 = states 序(成员添加序),节内行
  // 同排序管线(groupedSections 逐节 mixedRows);搜索过滤与混合模式同源
  // (按行键集合过滤,无搜索词时恒等)。
  const sections = useMemo(
    () => (viewOrg === "grouped" ? groupedSections(states, sortState, zhCmp()) : []),
    [viewOrg, states, sortState],
  );
  const filteredKeys = useMemo(() => new Set(rows.map((r) => r.key)), [rows]);
  const orderedKeys = useMemo(() => rows.map((r) => r.key), [rows]);
  // entry 引用 → 行:三视图回调只回传 FsEntry,引用回查补设备/复合键。
  const rowByEntry = useMemo(() => new Map(rows.map((r) => [r.entry, r])), [rows]);
  const keyOf = useCallback((e: FsEntry) => rowByEntry.get(e)?.key ?? e.path, [rowByEntry]);
  const colorOf = useCallback(
    (e: FsEntry) => {
      const row = rowByEntry.get(e);
      return row == null ? undefined : colors.get(row.device);
    },
    [rowByEntry, colors],
  );

  // Shift 范围段(与 dir 位同构:当前渲染序闭区间,更深底色区分)。
  const rangeSet = useMemo(() => {
    if (anchor == null || rangeEnd == null) return new Set<string>();
    const a = orderedKeys.indexOf(anchor);
    const b = orderedKeys.indexOf(rangeEnd);
    if (a < 0 || b < 0) return new Set<string>();
    return new Set(orderedKeys.slice(Math.min(a, b), Math.max(a, b) + 1));
  }, [anchor, rangeEnd, orderedKeys]);

  // 行数上抛(状态栏 fav 位 itemCount;行数变化即报)。
  useEffect(() => {
    onCount(rows.length);
  }, [onCount, rows.length]);

  // 渲染行上抛(T11 动作对等;渐进扇出逐帧更新,与 onCount 同款通道)。
  useEffect(() => {
    onRowsChange?.(rows);
  }, [onRowsChange, rows]);

  const handleRowClick = useCallback(
    (path: string, e: RowClickInfo) => {
      // 视图回调只回传 entry.path;复合键从事件宿主行的 data-row-path 取
      // (三视图恒渲染该属性;同 path 跨源两行据此消歧)。
      const key =
        (e as unknown as ReactMouseEvent<HTMLElement>).currentTarget?.dataset.rowPath ?? path;
      const mode = clickMode(e);
      const res = applyClickSelect(selected, orderedKeys, key, mode, anchor);
      onSelected(res.selection);
      setAnchor(res.anchor);
      setRangeEnd(mode === "shift" ? key : null);
    },
    [selected, orderedKeys, anchor, onSelected],
  );

  // 空白处点击清空选择(锚点/范围一并清,避免陈旧区间)。
  const handleClearSelect = useCallback(() => {
    onSelected(new Set<string>());
    setAnchor(null);
    setRangeEnd(null);
  }, [onSelected]);

  // 双击目录行钻取(混合只在第一层):补来源设备后交 App 门控导航。
  const handleOpenDir = useCallback(
    (entry: FsEntry) => {
      const row = rowByEntry.get(entry);
      if (row != null) onOpenDir(row.device, entry.path);
    },
    [rowByEntry, onOpenDir],
  );

  // 行右键上抛 MixedRow(T11 装配;本任务 App 只存不弹)。
  const handleRowContextMenu = useCallback(
    (entry: FsEntry, x: number, y: number) => {
      const row = rowByEntry.get(entry);
      if (row != null) onRowContextMenu(row, x, y);
    },
    [rowByEntry, onRowContextMenu],
  );

  // 渐进语义裁定:loading 恒 false(逐成员到齐即出行,不整段等待);
  // 空态判定按「已有 ok 成员」(全部在途/不可用时不显示「此文件夹为空」)。
  const listing = states.some((s) => s.status === "ok") ? { truncated: false } : null;

  // 离线/失败注记明细(title 悬停:设备名 + 路径,逐成员一行)。
  const unavailable = unavailableCount(states);
  const deviceName = (deviceId: string): string =>
    devices.find((d) => d.device_id === deviceId)?.name ?? deviceId;
  const unavailableTitle = states
    .filter((s) => s.status === "offline" || s.status === "failed")
    .map((s) => `${deviceName(s.member.device)} · ${s.member.path}`)
    .join("\n");

  // 分组节头素材(V3-3 Task 10):路径末段显示名(nav.pathSegments 分解
  // ——分隔符随目标平台,模块纪律:路径分解一律经 lib/nav,盘根/POSIX 根
  // 各自呈现 "C:" / "/");非 ok 态灰标注词键(离线/无法访问/加载中…,
  // ok → null 不渲染)。
  const pathTail = (p: string): string => {
    const segs = pathSegments(p);
    return segs.length > 0 ? segs[segs.length - 1] : p;
  };
  const statusTagKey = (status: MemberStatus): string | null =>
    status === "offline"
      ? "favorites.memberOffline"
      : status === "failed"
        ? "favorites.memberFailed"
        : status === "pending"
          ? "favorites.memberPending"
          : null;

  // 宿主拖放落点键(与侧栏快捷行同形,T12 消费:内部指针拖拽/Explorer
  // 拖入/悬停高亮三面同键解析在 App)。
  const dropKey = `fav\u0000${fav.id}`;

  // 节头标记语言(§14.4 起两分支共用:details 单表经 groupHeadInner 取
  // 内容,caret 仅视觉;list/tiles 逐节分支原样)。
  const groupHeadInner = (sec: GroupedSection, collapsed: boolean): ReactNode => {
    const dot = colors.get(sec.member.device);
    const tagKey = statusTagKey(sec.state.status);
    return (
      <>
        <span className={"caret" + (collapsed ? "" : " open")} aria-hidden="true">
          <IcChevronRight size={12} />
        </span>
        {dot != null && <span className="src-dot" style={{ background: dot }} />}
        <strong>{deviceName(sec.member.device)}</strong>
        <span className="fav-group-path">{pathTail(sec.member.path)}</span>
        {tagKey != null && <span className="fav-group-tag">{t(tagKey)}</span>}
      </>
    );
  };

  // 分组+details 节序列(§14.4 单表):节头样式/交互/落点契约原样装配,
  // 节内行过搜索过滤;收起节仅留节头(行集清空,选择集不修剪——§13.5
  // 裁定)。非该组合恒空数组(不传 sections = FileDetails 平铺模式)。
  const detailSections: DetailsSection[] =
    viewOrg === "grouped" && viewMode === "details"
      ? sections.map((sec) => {
          const secKey = memberKey(sec.member);
          const collapsed = collapsedKeys.has(secKey);
          const srows = sec.rows.filter((r) => filteredKeys.has(r.key));
          const online = devices.some((d) => d.device_id === sec.member.device && d.online);
          // 节级拖放落点键(在线节才有;T12 悬停高亮同 Sidebar DirNode
          // 机制:与 App 下发的 dropHoverKey 比对挂 .drop-hover)。
          const headDropKey = online ? `${sec.member.device}\u0000${sec.member.path}` : null;
          const unavail = sec.state.status === "offline" || sec.state.status === "failed";
          return {
            key: secKey,
            headClassName:
              "fav-group-head" +
              (unavail ? " unavailable" : "") +
              (collapsed ? " collapsed" : "") +
              (headDropKey != null && headDropKey === dropHoverKey ? " drop-hover" : ""),
            headDropLoc: headDropKey ?? undefined,
            onHeadClick: () => toggleSection(secKey),
            onHeadKeyDown: (e: ReactKeyboardEvent<HTMLDivElement>) => {
              if (e.key === "Enter" || e.key === " ") toggleSection(secKey);
            },
            head: groupHeadInner(sec, collapsed),
            rows: collapsed ? [] : srows.map((r) => r.entry),
          };
        })
      : [];

  const viewProps: DirViewProps = {
    rows: rows.map((r) => r.entry),
    loading: false, // 渐进裁定(见模块注)
    listing,
    error: null, // 逐成员失败入不可用注记,无全局错误面
    selected,
    cutPaths: NO_CUT, // T12 接线
    rangeSet,
    dragSourcePaths: dragSourcePaths ?? null, // T12:复合键集与 rowKey 同键比对
    searchActive: searchQ !== "",
    sortState,
    onSortKey,
    onRowClick: handleRowClick,
    onOpenDir: handleOpenDir,
    onClearSelect: handleClearSelect,
    onRetry: onRetry,
    onRowContextMenu: handleRowContextMenu,
    onBlankContextMenu: onBlankContextMenu,
    rowKey: keyOf,
    sourceColor: colorOf,
  };
  const View = viewMode === "details" ? FileDetails : viewMode === "list" ? FileList : FileTiles;

  // 不可用注记(两模式一致保留,T9 裁定;分组模式节头另有逐节灰标注)。
  const note =
    unavailable > 0 ? (
      <div className="fav-unavailable" title={unavailableTitle}>
        {t("favorites.unavailable", { count: unavailable })}
      </div>
    ) : null;

  return (
    <div
      className={"view-host" + (dropKey === dropHoverKey ? " drop-hover" : "")}
      data-drop-loc={dropKey}
      onPointerDown={onRowPointerDown}
    >
      {fav.members.length === 0 ? (
        <div className="fav-empty">{t("favorites.emptyHint")}</div>
      ) : viewOrg === "grouped" && viewMode === "details" ? (
        <>
          {/* §14.4:分组+details = 全部节并入一张表——表头全页恰一枚且
              粘性;节标签行(colSpan 行,.fav-group-head 原类/交互/落点
              契约)与节内行同表交错,列宽拖动与横滚跨节一致。listing 恒
              null(不渲染空态块,空/离线节只留节头,裁定同逐节分支)。
              list/tiles 无表头概念,仍逐节渲染(下方分支不动)。 */}
          <FileDetails {...viewProps} rows={[]} listing={null} sections={detailSections} />
          {note}
        </>
      ) : viewOrg === "grouped" ? (
        <>
          {sections.map((sec) => {
            const srows = sec.rows.filter((r) => filteredKeys.has(r.key));
            const online = devices.some((d) => d.device_id === sec.member.device && d.online);
            const secKey = memberKey(sec.member);
            const collapsed = collapsedKeys.has(secKey);
            // 节级拖放落点键(在线节才有;T12 悬停高亮同 Sidebar DirNode
            // 机制:与 App 下发的 dropHoverKey 比对挂 .drop-hover)。
            const headDropKey = online ? `${sec.member.device}\u0000${sec.member.path}` : null;
            const unavail = sec.state.status === "offline" || sec.state.status === "failed";
            return (
              <Fragment key={secKey}>
                {/* 节头(整行点击展开/收起,返工 §13.5;caret 仅视觉):
                    chevron+色点+设备名+路径末段+非 ok 态灰标注;在线节
                    宿主节级拖放落点 device\0path(T12 消费),收起也保留 */}
                <div
                  className={
                    "fav-group-head" +
                    (unavail ? " unavailable" : "") +
                    (collapsed ? " collapsed" : "") +
                    (headDropKey != null && headDropKey === dropHoverKey ? " drop-hover" : "")
                  }
                  data-drop-loc={headDropKey ?? undefined}
                  role="button"
                  tabIndex={0}
                  onClick={() => toggleSection(secKey)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") toggleSection(secKey);
                  }}
                >
                  {groupHeadInner(sec, collapsed)}
                </div>
                {/* 节内行:与混合模式同构渲染(复合键选择/色点/右键回调
                    同源);空节(ok 无条目)与 offline/failed/pending 节
                    只留节头标注,不渲染空态块(裁定);收起(§13.5)仅不
                    渲染行——选择集不修剪(裁定:状态栏计数仍在) */}
                {!collapsed && srows.length > 0 && (
                  <View {...viewProps} rows={srows.map((r) => r.entry)} listing={null} />
                )}
              </Fragment>
            );
          })}
          {note}
        </>
      ) : (
        <>
          <View {...viewProps} />
          {note}
        </>
      )}
    </div>
  );
}
