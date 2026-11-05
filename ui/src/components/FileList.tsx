/// 列表视图(单列名称):行图标(文件夹/类型角标页形)+ 名称,无其他列。
/// 行交互与详细信息视图同构:单击/Ctrl/Shift 多选(App 持有选择集与锚点)、
/// 双击目录行进入子目录(带多选修饰键的点击不触发)、点击空白处清空选择。
import type { FsEntry, SortKey, SortState } from "../lib/browse";
import { fileTypeName, fileTypeTintKey } from "../lib/filetype";
import { hasSelectModifier, type RowClickInfo } from "../lib/select";

import { IcFileTyped, IcFolder, type IconTintKey } from "./icons";
import DirStates from "./DirStates";

/// 目录三视图共享的行交互/状态契约(FileDetails/FileList/FileTiles 一致)。
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
  /// Shift 范围选择段(Task 13 打磨):锚点↔Shift 点击行闭区间的路径集。
  /// 命中且被选中的行以更深的 shift 底色呈现,与普通选中底色区分。
  rangeSet: ReadonlySet<string>;
  /// 正被拖拽的行路径集(Task 9 spec §9):拖拽成立时命中行挂 .drag-source
  /// 半透明(正被拖走的视觉标记);null = 无拖拽进行中。
  dragSourcePaths: ReadonlySet<string> | null;
  /// 搜索词是否激活(空列表时的空态文案区分:无匹配项/此文件夹为空)。
  searchActive: boolean;
  /// 当前排序状态(App 持有;三视图共享,经工具栏菜单或 details 表头修改)。
  sortState: SortState;
  /// 表头排序键点击(details 表头用;App 侧以 headerClick 合成新状态,
  /// list/tiles 无表头不消费)。
  onSortKey(key: SortKey): void;
  onRowClick(path: string, e: RowClickInfo): void;
  onOpenDir(entry: FsEntry): void;
  onClearSelect(): void;
  onRetry(): void;
  /// 右键菜单(Task 11):行上(调用方保证未选中行先单选)与容器空白处。
  onRowContextMenu(entry: FsEntry, x: number, y: number): void;
  onBlankContextMenu(x: number, y: number): void;
  /// 行身份键(缺省 = entry.path)。React key、data-row-path 与
  /// selected/rangeSet/cutPaths/dragSourcePaths 的成员判定全走此键——
  /// 混合视图跨设备同路径行以此区分(V3-3)。
  rowKey?: (entry: FsEntry) => string;
  /// 行首来源色点(缺省 = 不渲染)。返回 undefined = 该行无色点。
  sourceColor?: (entry: FsEntry) => string | undefined;
}

/// 三视图统一行图标(spec §8,16px):目录=文件夹;文件=页形+类型 tint
/// 角标(取色键由扩展名经 fileTypeTintKey 映射)。is_dir 先行分支后
/// tintKey 恒非 "folder"(filetype.ts 裁定),断言收窄至 IcFileTyped 的
/// IconTintKey;planner 裁定:「generic」是合法 tintKey,原样透传。
export function EntryIcon({ entry }: { entry: FsEntry }) {
  if (entry.is_dir) return <IcFolder size={16} />;
  return <IcFileTyped size={16} tintKey={fileTypeTintKey(entry.name, false) as IconTintKey} />;
}

export default function FileList({
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
  rowKey,
  sourceColor,
}: DirViewProps) {
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
      {rows.map((entry) => {
        // 行身份键逐行算一次(缺省 = path):React key、data-row-path 与
        // 各集合成员判定全走它;色点非 undefined 才渲染(行首图标前)。
        const key = rowKey ? rowKey(entry) : entry.path;
        const dot = sourceColor?.(entry);
        return (
          <div
            key={key}
            data-row-path={key}
            className={
              "file-row" +
              (entry.is_dir ? " dir" : " file") +
              (selected.has(key) ? " selected" : "") +
              (selected.has(key) && rangeSet.has(key) ? " shift-range" : "") +
              (cutPaths.has(key) ? " cut" : "") +
              (dragSourcePaths?.has(key) ? " drag-source" : "")
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
            {dot != null && (
              <span className="src-dot" style={{ background: dot }} aria-hidden="true" />
            )}
            <span className="file-icon" aria-hidden="true">
              <EntryIcon entry={entry} />
            </span>
            <span className="file-name">{entry.name}</span>
          </div>
        );
      })}
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
