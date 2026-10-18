/// 详细信息视图(目录浏览默认视图):名称/修改日期/类型/大小四列表格。
/// 行序由 sortEntries(listing, sortDir, zhCmp()) 决定(目录恒在前);
/// 单击行单选(选择集在 App 持有,Task 8 扩展 Ctrl/Shift);双击目录行
/// 进入子目录,双击文件无操作;加载/空/错误三态 + 截断提示行。
import { useMemo } from "react";
import { useTranslation } from "react-i18next";

import { fmtMtime, fmtSize, sortEntries, type FsEntry, type Listing } from "../lib/browse";
import { zhCmp } from "../lib/collate";

export default function FileDetails({
  loading,
  listing,
  error,
  sortDir,
  selected,
  onSelect,
  onOpenDir,
  onRetry,
}: {
  loading: boolean;
  listing: Listing | null;
  error: string | null;
  sortDir: "asc" | "desc";
  selected: Set<string>;
  onSelect(path: string): void;
  onOpenDir(entry: FsEntry): void;
  onRetry(): void;
}) {
  const { t } = useTranslation();
  // 真实 UI 默认 zhCmp()(拼音 collation);listing 不变则 memo 命中。
  const rows = useMemo(
    () => (listing == null ? [] : sortEntries(listing.entries, sortDir, zhCmp())),
    [listing, sortDir],
  );
  return (
    <div className="file-table-wrap">
      <table className="file-table">
        <thead>
          <tr>
            <th>{t("files.column.name")}</th>
            <th>{t("files.column.mtime")}</th>
            <th>{t("files.column.type")}</th>
            <th className="cell-size">{t("files.column.size")}</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((e) => (
            <tr
              key={e.path}
              className={
                (selected.has(e.path) ? "selected " : "") + (e.is_dir ? "dir" : "file")
              }
              onClick={() => onSelect(e.path)}
              onDoubleClick={() => {
                if (e.is_dir) onOpenDir(e);
              }}
            >
              <td className="cell-name">{e.name}</td>
              <td>{fmtMtime(e.mtime_ms)}</td>
              <td>{e.is_dir ? t("files.type.dir") : t("files.type.file")}</td>
              <td className="cell-size">{fmtSize(e.size, e.is_dir)}</td>
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
                {t("files.empty")}
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
