/// 平铺视图(图标+名称网格):自动换行的 tile 网格,行图标与列表/详细
/// 信息视图统一(EntryIcon,16px)。行交互与两视图同构:单击/Ctrl/Shift
/// 多选、双击目录行进入、空白处清空选择。
import { fileTypeName } from "../lib/filetype";
import { hasSelectModifier } from "../lib/select";

import DirStates from "./DirStates";
import { EntryIcon } from "./FileList";
import type { DirViewProps } from "./FileList";

export default function FileTiles({
  rows,
  loading,
  listing,
  error,
  selected,
  cutPaths,
  rangeSet,
  dragSourcePaths,
  searchActive,
  onRowClick,
  onOpenDir,
  onClearSelect,
  onRetry,
  onRowContextMenu,
  onBlankContextMenu,
}: DirViewProps) {
  return (
    <div
      className="file-tiles"
      onClick={(e) => {
        // 仅网格自身空白处点击清空;tile 内点击的冒泡目标是子元素。
        if (e.target === e.currentTarget) onClearSelect();
      }}
      onContextMenu={(e) => {
        // 网格空白处右键 → 空白菜单(粘贴/刷新);tile 右键交行处理器。
        if (e.target === e.currentTarget) {
          e.preventDefault();
          onBlankContextMenu(e.clientX, e.clientY);
        }
      }}
    >
      {rows.map((entry) => (
        <div
          key={entry.path}
          data-row-path={entry.path}
          className={
            "tile" +
            (entry.is_dir ? " dir" : " file") +
            (selected.has(entry.path) ? " selected" : "") +
            (selected.has(entry.path) && rangeSet.has(entry.path) ? " shift-range" : "") +
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
          title={fileTypeName(entry.name, entry.is_dir)}
        >
          <span className="tile-icon" aria-hidden="true">
            <EntryIcon entry={entry} />
          </span>
          <span className="tile-name">{entry.name}</span>
        </div>
      ))}
      <DirStates
        loading={loading}
        error={error}
        hasListing={listing != null}
        rowCount={rows.length}
        searchActive={searchActive}
        truncated={listing?.truncated ?? false}
        onRetry={onRetry}
      />
    </div>
  );
}
