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
  /// 内部剪贴板为剪切时的源行路径集(空集 = 复制/无剪贴板):行灰显,
  /// 资源管理器剪切同款视觉标记(Task 12)。
  cutPaths: ReadonlySet<string>;
  /// 搜索词是否激活(空列表时的空态文案区分:无匹配项/此文件夹为空)。
  searchActive: boolean;
  onRowClick(path: string, e: RowClickInfo): void;
  onOpenDir(entry: FsEntry): void;
  onClearSelect(): void;
  onRetry(): void;
  /// 右键菜单(Task 11):行上(调用方保证未选中行先单选)与容器空白处。
  onRowContextMenu(entry: FsEntry, x: number, y: number): void;
  onBlankContextMenu(x: number, y: number): void;
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
  cutPaths,
  searchActive,
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
      className="file-list"
      onClick={(e) => {
        // 仅容器自身的空白区域点击清空;行内点击经冒泡目标是子元素。
        if (e.target === e.currentTarget) onClearSelect();
      }}
      onContextMenu={(e) => {
        // 容器空白处右键 → 空白菜单(粘贴/刷新);行右键交行处理器。
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
            "file-row" +
            (entry.is_dir ? " dir" : " file") +
            (selected.has(entry.path) ? " selected" : "") +
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
        searchActive={searchActive}
        truncated={listing?.truncated ?? false}
        onRetry={onRetry}
      />
    </div>
  );
}
