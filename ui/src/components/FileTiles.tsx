/// 平铺视图(图标+名称网格):自动换行的 tile 网格。行交互与列表/详细
/// 信息视图同构:单击/Ctrl/Shift 多选、双击目录行进入、空白处清空选择。
import { useTranslation } from "react-i18next";

import type { FsEntry } from "../lib/browse";
import { hasSelectModifier } from "../lib/select";

import DirStates from "./DirStates";
import type { DirViewProps } from "./FileList";

/// 条目图标字形:与列表视图一致(目录 ▸ / 文件 ▪),tile 下放大呈现。
function iconFor(e: FsEntry): string {
  return e.is_dir ? "▸" : "▪";
}

export default function FileTiles({
  rows,
  loading,
  listing,
  error,
  selected,
  searchActive,
  onRowClick,
  onOpenDir,
  onClearSelect,
  onRetry,
}: DirViewProps) {
  const { t } = useTranslation();
  return (
    <div
      className="file-tiles"
      onClick={(e) => {
        // 仅网格自身空白处点击清空;tile 内点击的冒泡目标是子元素。
        if (e.target === e.currentTarget) onClearSelect();
      }}
    >
      {rows.map((entry) => (
        <div
          key={entry.path}
          className={
            "tile" +
            (entry.is_dir ? " dir" : " file") +
            (selected.has(entry.path) ? " selected" : "")
          }
          onClick={(e) => onRowClick(entry.path, e)}
          onDoubleClick={(e) => {
            if (hasSelectModifier(e)) return; // 多选手势不触发打开
            if (entry.is_dir) onOpenDir(entry);
          }}
          title={entry.is_dir ? t("files.type.dir") : t("files.type.file")}
        >
          <span className="tile-icon" aria-hidden="true">
            {iconFor(entry)}
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
