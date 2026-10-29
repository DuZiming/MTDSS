/// 详细信息视图(目录浏览默认视图):名称/修改日期/类型/大小四列表格,
/// 四列均为可点排序表头(同键翻 dir/新键 asc,经 App 的 headerClick 合成;
/// 单击表头不触碰选择)。行序由 App 层统一计算(sortEntries ∘ filterEntries
/// 后的 rows 传入);行图标与列表/平铺视图统一(EntryIcon,16px);类型列
/// 走 fileTypeName 词表(与排序 type 键同源)。单击/Ctrl/Shift 多选(选择集
/// 与锚点在 App,applyClickSelect 解析);双击目录行进入子目录(带多选修饰
/// 键的点击不触发),双击文件无操作;加载/空/错误三态 + 截断提示行(表格内
/// 嵌,与其他视图的 DirStates 同文案)。
/// r2b 列宽拖动,r2d 改「调谁动谁」模型:表头右缘拖柄(纯指针操作,
/// aria-hidden)横向拖调列宽,自定义 px 由 colwidths.ts clamp 并持久于
/// localStorage["mtdss.colw"]。四列各持独立 px——拖哪列只改哪列,其余
/// 纹丝不动;名称列也持久化(有值才存,未拖过保持弹性吃剩余空间)。
/// 表格 min-width = sumColWidths(四列当前宽之和),容器横向可滚:总和
/// 超出可视宽出滚动条不挤压;容器够宽时表格 100% 拉伸、富余由名称列
/// 吸收(名称已钉时表宽=四列和、右缘留白,保持其余列 px 分毫不差——
/// 两模式实现取简,已记档)。拖动中表体即时跟随(同一 state),pointerup
/// 落盘;拖柄 stopPropagation,不触发排序。
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
  DEFAULT_COL_WIDTHS,
  clampColWidth,
  loadColWidths,
  saveColWidths,
  sumColWidths,
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

/// 一次列宽拖动会话:pointerdown 建档(起点/当前宽),pointermove 逐次
/// 覆写当前宽,pointerup/cancel 清档落盘。「调谁动谁」无列间联动。
interface ColDrag {
  key: ColKey;
  startX: number;
  startW: number;
  /// 会话内最新已应用宽(按下即=startW)。落盘以此为准——同一任务里
  /// 连发 move/up 时,up 的 state 闭包可能尚未含末次 move 的 setState,
  /// 从会话档取值才确定。
  w: number;
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

  /// 已拖过(自定义)的列宽(px,含名称列);未拖过的列:其余三列走
  /// DEFAULT_COL_WIDTHS 基线,名称保持弹性。
  const [colW, setColW] = useState<ColWidths>(() => loadColWidths());
  const dragRef = useRef<ColDrag | null>(null);

  /// 其余三列生效宽:自定义 px 优先,否则默认基线(名称列不在内)。
  const colWidth = (key: Exclude<ColKey, "name">): number =>
    colW[key] ?? DEFAULT_COL_WIDTHS[key];

  /// 表格宽度两模式(r2d):名称未钉 → minWidth=四列和,表格 100% 拉伸、
  /// 富余由名称吸收;名称已钉 → 表宽=四列和(右缘留白),四列 px 分毫
  /// 不差、不随窗口拉伸(否则固定布局会把富余按比例摊进各列,破坏「调
  /// 谁动谁」;两模式实现取简,已记档)。容器 overflow-x 兜横向滚动。
  const tableW = sumColWidths(colW);

  /// 拖柄 pointerdown:以「当前视觉宽」建档(其余列=生效 px;名称未钉
  /// 时=弹性现值,拖动即从所见边界起步,起拖零跳变)。
  const startColDrag =
    (key: ColKey) => (e: ReactPointerEvent<HTMLSpanElement>) => {
      if (!e.isPrimary || dragRef.current != null) return;
      const th = e.currentTarget.closest("th");
      if (th == null) return;
      e.preventDefault();
      e.stopPropagation(); // 拖柄在表头内:指针按下不冒泡成排序点击
      const startW = th.getBoundingClientRect().width;
      dragRef.current = { key, startX: e.clientX, startW, w: startW };
      // 捕获保证移出 6px 命中区仍持续跟踪;合成事件(自动化)无真实指针
      // 会抛 NotFoundError,吞掉不影响建档,后续事件按派发目标照常到达。
      try {
        e.currentTarget.setPointerCapture(e.pointerId);
      } catch {
        // 无真实指针 → 跳过捕获
      }
    };

  /// 拖柄 pointermove:横向增量换算本列新宽,逐列 clamp 后写 state——
  /// 「调谁动谁」,其余列宽度不受影响;会话档同步记最新值供落盘。
  const moveColDrag = (e: ReactPointerEvent<HTMLSpanElement>) => {
    const d = dragRef.current;
    if (d == null) return;
    const w = clampColWidth(d.key, d.startW + (e.clientX - d.startX));
    d.w = w;
    setColW((prev) => ({ ...prev, [d.key]: w }));
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
    if (d.w !== d.startW) saveColWidths({ ...colW, [d.key]: d.w });
  };

  /// 拖柄事件组(四列同构,均直调本列);click 亦拦——拖完抬指落在表头
  /// 会合成 click,不拦会翻排序。
  const gripProps = (key: ColKey): HTMLAttributes<HTMLSpanElement> => ({
    onPointerDown: startColDrag(key),
    onPointerMove: moveColDrag,
    onPointerUp: endColDrag,
    onPointerCancel: endColDrag,
    onClick: (e: ReactMouseEvent) => e.stopPropagation(),
  });

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
      <table
        className="file-table"
        style={
          colW.name != null
            ? { width: tableW, minWidth: tableW }
            : { minWidth: tableW }
        }
      >
        <thead>
          <tr>
            {COLUMNS.map(({ key, label }) => {
              const active = sortState.key === key;
              const w = key === "name" ? colW.name : colWidth(key);
              return (
                <th
                  key={key}
                  className={"sortable" + (active ? " active" : "") + (key === "size" ? " cell-size" : "")}
                  style={w != null ? { width: w } : undefined}
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
