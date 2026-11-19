/// 平铺视图(V4-1 T19 Explorer 式重做):图标槽(aspect-1 等比缩放居中
/// 不裁切)+ 名称 + 第二行小字(目录=修改日期,文件=「类型 · 大小」)。
/// 图/视频行回环口真缩略图(EntryThumb,失败回退类型图标);文档/压缩包
/// /音频=类型淡染页形——icons.tsx 存档 SVG 原样放大(78/66 存档 m 档锚,
/// CSS 变量承载尺寸),不自创笔画。三档尺寸=data-tile-size 落 App.css
/// 三档 --tile-* 变量,Ctrl+滚轮翻档(lib/tilesize 纯函数;档位 App 会话
/// 态持有,与 sortState 同款「仅内存态」先例)。悬停浮出:选择圈(Ctrl
/// 切换选中,选中恒显)+ 快捷下载(远端文件行;App 仅在远端设备目录位
/// 提供处理器,本机文件无「下载」语义、fav 混合视图走跨源分组流不接线)。
/// 行交互与列表/详细信息视图同构零回归:单击/Ctrl/Shift 多选、双击目录
/// 进入、右键菜单、剪切高亮、拖拽源、空白清选。
import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";

import { fmtMtime, fmtSize, type FsEntry } from "../lib/browse";
import { fileTypeName } from "../lib/filetype";
import { hasSelectModifier } from "../lib/select";
import { TILE_THUMB_W, stepTileSize } from "../lib/tilesize";

import DirStates from "./DirStates";
import EntryThumb from "./EntryThumb";
import { IcDirRecv } from "./icons";
import type { DirViewProps } from "./FileList";

/// 第二行小字(数据源=现有 entry 字段;格式化沿仓内先例 fmtMtime/
/// fileTypeName/fmtSize):目录=修改日期(「N 项」数据源 child_count 不在
/// 浏览切片,V3-6b 起桌面浏览计数已停,故仅日期);文件=「类型 · 大小」。
/// 空段(mtime=0/目录大小空串)剔除,不落悬空「·」分隔。
export function tileMetaLine(entry: FsEntry): string {
  if (entry.is_dir) return fmtMtime(entry.mtime_ms);
  return [fileTypeName(entry.name, false), fmtSize(entry.size, false)]
    .filter((s) => s !== "")
    .join(" · ");
}

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
  rowKey,
  thumbDevices,
  tileSize = "m",
  onTileSize,
  onQuickDownload,
}: DirViewProps) {
  const { t } = useTranslation();
  const gridRef = useRef<HTMLDivElement | null>(null);
  /// 档位镜像:原生滚轮监听读现值,免逐档重挂监听。
  const sizeRef = useRef(tileSize);
  sizeRef.current = tileSize;
  // Ctrl+滚轮翻档走原生非 passive 监听(React 根级 wheel 监听为 passive,
  // preventDefault 无效且告警;此处须压下浏览器页缩放让位给翻档)。
  useEffect(() => {
    const el = gridRef.current;
    if (el == null || onTileSize == null) return;
    const onWheel = (e: WheelEvent): void => {
      if (!e.ctrlKey) return; // 纯滚轮=原生滚动,不翻档
      e.preventDefault();
      onTileSize(stepTileSize(sizeRef.current, e.deltaY));
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [onTileSize]);
  return (
    <div
      ref={gridRef}
      className="file-tiles"
      data-tile-size={tileSize}
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
      {rows.map((entry, i) => {
        // 行身份键逐行算一次(缺省 = path):React key、data-row-path 与
        // 各集合成员判定全走它。
        const key = rowKey ? rowKey(entry) : entry.path;
        return (
          <div
            key={key}
            data-row-path={key}
            className={
              "tile" +
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
            {/* 悬停浮出选择圈:Ctrl 切换选中(与 Ctrl+点击行同语义);选中
                恒显。stopPropagation 防与行点击/双击重复触发。 */}
            <button
              type="button"
              className="tile-check"
              aria-label={t("files.tileSelect")}
              aria-pressed={selected.has(key)}
              title={t("files.tileSelect")}
              onClick={(e) => {
                e.stopPropagation();
                onRowClick(entry.path, { shiftKey: false, ctrlKey: true, metaKey: false });
              }}
              onDoubleClick={(e) => e.stopPropagation()}
            />
            {/* 悬停浮出快捷下载(远端文件行;处理器缺省不呈现) */}
            {onQuickDownload != null && !entry.is_dir && (
              <button
                type="button"
                className="tile-download"
                aria-label={t("files.quickDownload")}
                title={t("files.quickDownload")}
                onClick={(e) => {
                  e.stopPropagation();
                  onQuickDownload(entry);
                }}
                onDoubleClick={(e) => e.stopPropagation()}
              >
                <IcDirRecv size={12} />
              </button>
            )}
            {/* 图标槽:aspect-1 等比居中不裁切;EntryThumb 失败/空回退
                EntryIcon(icons.tsx 存档笔画原样放大) */}
            <span className="tile-slot" aria-hidden="true">
              <EntryThumb
                entry={entry}
                device={thumbDevices?.[i] ?? null}
                w={TILE_THUMB_W[tileSize]}
              />
            </span>
            <span className="tile-name">{entry.name}</span>
            <span className="tile-meta">{tileMetaLine(entry)}</span>
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
