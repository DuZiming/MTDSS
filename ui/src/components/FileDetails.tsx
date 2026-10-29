/// 详细信息视图(目录浏览默认视图):名称/修改日期/类型/大小四列表格,
/// 四列均为可点排序表头(同键翻 dir/新键 asc,经 App 的 headerClick 合成;
/// 单击表头不触碰选择)。行序由 App 层统一计算(sortEntries ∘ filterEntries
/// 后的 rows 传入);行图标与列表/平铺视图统一(EntryIcon,16px);类型列
/// 走 fileTypeName 词表(与排序 type 键同源)。单击/Ctrl/Shift 多选(选择集
/// 与锚点在 App,applyClickSelect 解析);双击目录行进入子目录(带多选修饰
/// 键的点击不触发),双击文件无操作;加载/空/错误三态 + 截断提示行(表格内
/// 嵌,与其他视图的 DirStates 同文案)。
import { useTranslation } from "react-i18next";

import { fmtMtime, fmtSize, type FsEntry, type SortKey } from "../lib/browse";
import { fileTypeName } from "../lib/filetype";
import { hasSelectModifier } from "../lib/select";

import { EntryIcon, type DirViewProps } from "./FileList";

/// 表头四列的展示序与 i18n 键(与 spec §6 排序菜单四键同序)。
const COLUMNS: readonly { key: SortKey; label: string }[] = [
  { key: "name", label: "files.column.name" },
  { key: "mtime", label: "files.column.mtime" },
  { key: "type", label: "files.column.type" },
  { key: "size", label: "files.column.size" },
];

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
      <table className="file-table">
        <thead>
          <tr>
            {COLUMNS.map(({ key, label }) => {
              const active = sortState.key === key;
              return (
                <th
                  key={key}
                  className={"sortable" + (active ? " active" : "") + (key === "size" ? " cell-size" : "")}
                  onClick={() => onSortKey(key)}
                  aria-sort={active ? (sortState.dir === "asc" ? "ascending" : "descending") : undefined}
                >
                  <span className="th-in">
                    {t(label)}
                    <span className="th-sort" aria-hidden="true">
                      {active ? <SortTri dir={sortState.dir} /> : <SortTriUpDown />}
                    </span>
                  </span>
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
                (cutPaths.has(entry.path) ? " cut" : "")
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
                {entry.name}
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
