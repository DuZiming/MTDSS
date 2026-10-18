/// 详细信息视图(目录浏览默认视图):名称/修改日期/类型/大小四列表格。
/// 行序由 App 层统一计算(sortEntries ∘ filterEntries 后的 rows 传入);
/// 单击/Ctrl/Shift 多选(选择集与锚点在 App,applyClickSelect 解析);
/// 双击目录行进入子目录(带多选修饰键的点击不触发),双击文件无操作;
/// 加载/空/错误三态 + 截断提示行(表格内嵌,与其他视图的 DirStates 同文案)。
import { useTranslation } from "react-i18next";

import { fmtMtime, fmtSize, type FsEntry } from "../lib/browse";
import { hasSelectModifier } from "../lib/select";

import type { DirViewProps } from "./FileList";

export default function FileDetails({
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
      className="file-table-wrap"
      onClick={(e) => {
        // 表格外剩余空白处点击清空选择(表格自身占满宽度,空白在其下方)。
        if (e.target === e.currentTarget) onClearSelect();
      }}
    >
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
          {rows.map((entry: FsEntry) => (
            <tr
              key={entry.path}
              className={
                (selected.has(entry.path) ? "selected " : "") +
                (entry.is_dir ? "dir" : "file")
              }
              onClick={(e) => onRowClick(entry.path, e)}
              onDoubleClick={(e) => {
                if (hasSelectModifier(e)) return; // 多选手势不触发打开
                if (entry.is_dir) onOpenDir(entry);
              }}
            >
              <td className="cell-name">{entry.name}</td>
              <td>{fmtMtime(entry.mtime_ms)}</td>
              <td>{entry.is_dir ? t("files.type.dir") : t("files.type.file")}</td>
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
