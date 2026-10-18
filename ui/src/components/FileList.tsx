/// 列表视图(单列名称):图标字形 + 名称,无其他列。行交互与详细信息
/// 视图同构:单击/Ctrl/Shift 多选(App 持有选择集与锚点)、双击目录行
/// 进入子目录(带多选修饰键的点击不触发)、点击空白处清空选择。
import { useTranslation } from "react-i18next";

import type { FsEntry } from "../lib/browse";
import { hasSelectModifier, type RowClickInfo } from "../lib/select";

import DirStates from "./DirStates";

/// 目录三视图共享的行交互契约(FileDetails/FileList/FileTiles 一致)。
export interface DirViewProps {
  /// 当前渲染序(排序 + 过滤后)的条目。
  rows: FsEntry[];
  loading: boolean;
  listing: { truncated: boolean } | null;
  error: string | null;
  selected: ReadonlySet<string>;
  onRowClick(path: string, e: RowClickInfo): void;
  onOpenDir(entry: FsEntry): void;
  onClearSelect(): void;
  onRetry(): void;
}

/// 条目图标字形:目录沿用树的 caret 语言(▸),文件用实心小方块;
/// 纯 unicode 几何字形,无 emoji,随文本色灰显。
function iconFor(e: FsEntry): string {
  return e.is_dir ? "▸" : "▪";
}

export default function FileList({
  rows,
  loading,
  listing,
  error,
  selected,
  onRowClick,
  onOpenDir,
  onClearSelect,
  onRetry,
}: DirViewProps) {
  const { t } = useTranslation();
  return (
    <div
      className="file-list"
      onClick={(e) => {
        // 仅容器自身的空白区域点击清空;行内点击经冒泡目标是子元素。
        if (e.target === e.currentTarget) onClearSelect();
      }}
    >
      {rows.map((entry) => (
        <div
          key={entry.path}
          className={
            "file-row" +
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
          <span className="file-icon" aria-hidden="true">
            {iconFor(entry)}
          </span>
          <span className="file-name">{entry.name}</span>
        </div>
      ))}
      <DirStates
        loading={loading}
        error={error}
        hasListing={listing != null}
        rowCount={rows.length}
        truncated={listing?.truncated ?? false}
        onRetry={onRetry}
      />
    </div>
  );
}
