/// 详细信息视图(目录浏览默认视图):名称/修改日期/类型/大小四列表格,
/// 四列均为可点排序表头(同键翻 dir/新键 asc,经 App 的 headerClick 合成;
/// 单击表头不触碰选择)。行序由 App 层统一计算(sortEntries ∘ filterEntries
/// 后的 rows 传入);行图标与列表/平铺视图统一(EntryIcon,16px);类型列
/// 走 fileTypeName 词表(与排序 type 键同源)。单击/Ctrl/Shift 多选(选择集
/// 与锚点在 App,applyClickSelect 解析);双击目录行进入子目录(带多选修饰
/// 键的点击不触发),双击文件无操作;加载/空/错误三态 + 截断提示行(表格内
/// 嵌,与其他视图的 DirStates 同文案)。
/// r2b 列宽拖动:表头右缘拖柄(纯指针操作,aria-hidden)横向拖调列宽,
/// 自定义 px 由 colwidths.ts clamp 并持久于 localStorage["mtdss.colw"]。
/// mtime/type/size 存自定义 px、未拖过走默认基线(170/130/90);名称列
/// 恒弹性不存宽(固定布局下吃剩余宽,名称缘拖动反向驱动邻列 mtime,其
/// 下限由 CSS min-width + 拖动动态上限双保险)。拖动中表体即时跟随
/// (同一 state),pointerup 落盘;拖柄 stopPropagation,不触发排序。
import { useRef, useState } from "react";
import type {
  HTMLAttributes,
  MouseEvent as ReactMouseEvent,
  PointerEvent as ReactPointerEvent,
} from "react";
import { useTranslation } from "react-i18next";

import { fmtMtime, fmtSize, type FsEntry, type SortKey } from "../lib/browse";
import { fileTypeName } from "../lib/filetype";
import { hasSelectModifier } from "../lib/select";
import {
  COL_MIN_WIDTHS,
  DEFAULT_COL_WIDTHS,
  clampColWidth,
  loadColWidths,
  saveColWidths,
  type ColKey,
  type ColWidths,
} from "../lib/colwidths";

import { EntryIcon, type DirViewProps } from "./FileList";

/// 表头四列的展示序与 i18n 键(与 spec §6 排序菜单四键同序)。
const COLUMNS: readonly { key: SortKey; label: string }[] = [
  { key: "name", label: "files.column.name" },
  { key: "mtime", label: "files.column.mtime" },
  { key: "type", label: "files.column.type" },
  { key: "size", label: "files.column.size" },
];

/// 存 px 的三列(名称列恒弹性,见文件头 r2b 裁定)。
const PX_COLS: readonly Exclude<ColKey, "name">[] = ["mtime", "type", "size"];

/// 一次列宽拖动会话:pointerdown 建档(pointermove 起点/动态上限/当前宽),
/// pointerup/cancel 清档落盘。invert = 名称缘拖柄(反向驱动邻列 mtime)。
interface ColDrag {
  target: Exclude<ColKey, "name">;
  invert: boolean;
  startX: number;
  startW: number;
  /// 会话内最新已应用宽(指针按下即=startW;move 逐次覆写)。落盘以此
  /// 为准——同一任务里连发 move/up 时,up 的 state 闭包可能尚未含末次
  /// move 的 setState,从会话档取值才确定。
  w: number;
  maxW: number;
}

/// 排序方向实心三角(spec §8「活动列实心 ▲/▼」):icons.tsx 的导出面被
/// 合同测试钉死(不可新增导出),线性族又无实心形,故按 task 约定在消费
/// 侧内联最小实心三角——fill=currentColor 随文本色,零字面色,纪律与
/// 图标库一致(此裁定记档)。
function SortTri({ dir }: { dir: "asc" | "desc" }) {
  return (
    <svg width={8} height={8} viewBox="0 0 8 8" aria-hidden="true">
      {dir === "asc" ? (
        <path d="M4 1 7.4 6.9H.6z" fill="currentColor" />
      ) : (
        <path d="M4 6.9 .6 1h6.8z" fill="currentColor" />
      )}
    </svg>
  );
}

/// 非活动列悬停的淡 ↕ 提示(上下双实心三角);内联纪律同 SortTri。
function SortTriUpDown() {
  return (
    <svg width={8} height={10} viewBox="0 0 8 10" aria-hidden="true">
      <path d="M4 0 7 4.3H1z" fill="currentColor" />
      <path d="M4 10 1 5.7h6z" fill="currentColor" />
    </svg>
  );
}

export default function FileDetails({
  rows,
  loading,
  listing,
  error,
  selected,
  cutPaths,
  rangeSet,
  dragSourcePaths,
  searchActive,
  sortState,
  onSortKey,
  onRowClick,
  onOpenDir,
  onClearSelect,
  onRetry,
  onRowContextMenu,
  onBlankContextMenu,
}: DirViewProps) {
  const { t } = useTranslation();

  /// 已拖过(自定义)的列宽;未拖过的列走 DEFAULT_COL_WIDTHS 基线。
  const [colW, setColW] = useState<ColWidths>(() => loadColWidths());
  const tableRef = useRef<HTMLTableElement | null>(null);
  const dragRef = useRef<ColDrag | null>(null);

  /// 列宽生效值:自定义 px 优先,否则默认基线(名称列恒无固定宽,不在内)。
  const colWidth = (key: Exclude<ColKey, "name">): number =>
    colW[key] ?? DEFAULT_COL_WIDTHS[key];

  /// 拖柄 pointerdown:以当前生效宽建档;动态上限 = 表宽 − 名称保底 −
  /// 其余两列生效宽,保证名称列不被其余列挤穿(COL_MIN_WIDTHS.name)。
  const startColDrag =
    (target: Exclude<ColKey, "name">, invert: boolean) =>
    (e: ReactPointerEvent<HTMLSpanElement>) => {
      if (!e.isPrimary || dragRef.current != null) return;
      const table = tableRef.current;
      if (table == null) return;
      e.preventDefault();
      e.stopPropagation(); // 拖柄在表头内:指针按下不冒泡成排序点击
      const otherUsed = PX_COLS.filter((k) => k !== target).reduce(
        (s, k) => s + colWidth(k),
        0,
      );
      dragRef.current = {
        target,
        invert,
        startX: e.clientX,
        startW: colWidth(target),
        w: colWidth(target),
        maxW: Math.max(
          COL_MIN_WIDTHS[target],
          table.getBoundingClientRect().width - COL_MIN_WIDTHS.name - otherUsed,
        ),
      };
      // 捕获保证移出 6px 命中区仍持续跟踪;合成事件(自动化)无真实指针
      // 会抛 NotFoundError,吞掉不影响建档,后续事件按派发目标照常到达。
      try {
        e.currentTarget.setPointerCapture(e.pointerId);
      } catch {
        // 无真实指针 → 跳过捕获
      }
    };

  /// 拖柄 pointermove:横向增量换算新宽(名称缘 invert 取反向——名称列
  /// 恒弹性,边界物理位移等价于邻列 mtime 宽的反向增减),逐列 clamp 后
  /// 写 state,表头表体同源即时跟随;会话档同步记最新值供落盘。
  const moveColDrag = (e: ReactPointerEvent<HTMLSpanElement>) => {
    const d = dragRef.current;
    if (d == null) return;
    const dx = e.clientX - d.startX;
    const raw = d.invert ? d.startW - dx : d.startW + dx;
    const w = Math.min(d.maxW, clampColWidth(d.target, raw));
    d.w = w;
    setColW((prev) => ({ ...prev, [d.target]: w }));
  };

  /// 拖柄 pointerup/cancel:清档并落盘。落盘值 = 既有自定义 + 会话档
  /// 最新宽(同一任务连发 move/up 时 state 闭包不含末次 move,以档为准;
  /// 其余键未被动过,闭包值即最新)。写失败静默,当次会话仍生效。
  const endColDrag = (e: ReactPointerEvent<HTMLSpanElement>) => {
    const d = dragRef.current;
    if (d == null) return;
    dragRef.current = null;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
    if (d.w !== d.startW) saveColWidths({ ...colW, [d.target]: d.w });
  };

  /// 拖柄事件组(name 缘反向驱动 mtime,其余直接驱动本列);click 亦拦
  /// ——拖完抬指落在表头会合成 click,不拦会翻排序。
  const gripProps = (key: ColKey): HTMLAttributes<HTMLSpanElement> => {
    const [target, invert] =
      key === "name" ? (["mtime", true] as const) : ([key, false] as const);
    return {
      onPointerDown: startColDrag(target, invert),
      onPointerMove: moveColDrag,
      onPointerUp: endColDrag,
      onPointerCancel: endColDrag,
      onClick: (e: ReactMouseEvent) => e.stopPropagation(),
    };
  };

  return (
    <div
      className="file-table-wrap"
      onClick={(e) => {
        // 表格外剩余空白处点击清空选择(表格自身占满宽度,空白在其下方)。
        if (e.target === e.currentTarget) onClearSelect();
      }}
      onContextMenu={(e) => {
        // 表格下方空白处右键 → 空白菜单(粘贴/刷新);行右键交行处理器。
        if (e.target === e.currentTarget) {
          e.preventDefault();
          onBlankContextMenu(e.clientX, e.clientY);
        }
      }}
    >
      <table ref={tableRef} className="file-table">
        <thead>
          <tr>
            {COLUMNS.map(({ key, label }) => {
              const active = sortState.key === key;
              return (
                <th
                  key={key}
                  className={
                    "sortable" +
                    (active ? " active" : "") +
                    (key === "name" ? " cell-name" : "") +
                    (key === "size" ? " cell-size" : "")
                  }
                  style={key === "name" ? undefined : { width: colWidth(key) }}
                  onClick={() => onSortKey(key)}
                  aria-sort={active ? (sortState.dir === "asc" ? "ascending" : "descending") : undefined}
                >
                  <span className="th-in">
                    {t(label)}
                    <span className="th-sort" aria-hidden="true">
                      {active ? <SortTri dir={sortState.dir} /> : <SortTriUpDown />}
                    </span>
                  </span>
                  <span
                    className="col-grip"
                    aria-hidden="true"
                    {...gripProps(key)}
                  />
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody>
          {rows.map((entry: FsEntry) => (
            <tr
              key={entry.path}
              data-row-path={entry.path}
              className={
                (selected.has(entry.path) ? "selected " : "") +
                (selected.has(entry.path) && rangeSet.has(entry.path) ? "shift-range " : "") +
                (entry.is_dir ? "dir" : "file") +
                (cutPaths.has(entry.path) ? " cut" : "") +
                (dragSourcePaths?.has(entry.path) ? " drag-source" : "")
              }
              onClick={(e) => onRowClick(entry.path, e)}
              onDoubleClick={(e) => {
                if (hasSelectModifier(e)) return; // 多选手势不触发打开
                if (entry.is_dir) onOpenDir(entry);
              }}
              onContextMenu={(e) => {
                e.preventDefault();
                onRowContextMenu(entry, e.clientX, e.clientY);
              }}
            >
              <td className="cell-name">
                <EntryIcon entry={entry} />
                {/* r2b:名称文本收进 .fname,截断点在名称 span(省略号),
                 * 不截整行、不吞图标 */}
                <span className="fname">{entry.name}</span>
              </td>
              <td>{fmtMtime(entry.mtime_ms)}</td>
              <td>{fileTypeName(entry.name, entry.is_dir)}</td>
              <td className="cell-size">{fmtSize(entry.size, entry.is_dir)}</td>
            </tr>
          ))}
          {loading && (
            <tr>
              <td colSpan={4} className="state">
                {t("files.loading")}
              </td>
            </tr>
          )}
          {!loading && error != null && (
            <tr>
              <td colSpan={4} className="state">
                {t("files.error")}:{error}
                <div>
                  <button className="btn" onClick={onRetry}>
                    {t("common.retry")}
                  </button>
                </div>
              </td>
            </tr>
          )}
          {!loading && error == null && listing != null && rows.length === 0 && (
            <tr>
              <td colSpan={4} className="state">
                {searchActive ? t("files.noMatch") : t("files.empty")}
              </td>
            </tr>
          )}
          {listing?.truncated && (
            <tr className="truncated-row">
              <td colSpan={4}>{t("files.truncated")}</td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}
