/// 目录视图公共状态块(列表/平铺视图共用):加载中 / 错误+重试 / 空 /
/// 截断提示。详细信息视图因表格结构内嵌同构状态行,不复用本组件。
import { useTranslation } from "react-i18next";

export default function DirStates({
  loading,
  error,
  hasListing,
  rowCount,
  searchActive,
  truncated,
  onRetry,
}: {
  loading: boolean;
  error: string | null;
  /// 是否已有列表快照(区分「尚未加载」与「确认为空」)。
  hasListing: boolean;
  rowCount: number;
  /// 搜索词是否激活(空态文案随之区分:无匹配项 vs 此文件夹为空)。
  searchActive: boolean;
  truncated: boolean;
  onRetry(): void;
}) {
  const { t } = useTranslation();
  const emptyVisible = !loading && error == null && hasListing && rowCount === 0;
  if (!loading && error == null && !emptyVisible && !truncated) return null;
  return (
    <div className="dir-states">
      {loading && <div className="dir-state">{t("files.loading")}</div>}
      {!loading && error != null && (
        <div className="dir-state">
          {t("files.error")}:{error}
          <div>
            <button className="btn" onClick={onRetry}>
              {t("common.retry")}
            </button>
          </div>
        </div>
      )}
      {emptyVisible && (
        <div className="dir-state">
          {searchActive ? t("files.noMatch") : t("files.empty")}
        </div>
      )}
      {truncated && <div className="dir-truncated">{t("files.truncated")}</div>}
    </div>
  );
}
