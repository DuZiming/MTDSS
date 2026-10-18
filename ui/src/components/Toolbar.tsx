/// 工具栏:设计 §3 的按钮行。M3a 全部为 disabled 占位,
/// 实际行为(新建内容/剪贴板/排序/视图切换)V2 再定。
import { useTranslation } from "react-i18next";

export default function Toolbar() {
  const { t } = useTranslation();
  return (
    <div className="toolbar" role="toolbar" aria-label={t("toolbar.new")}>
      <button className="tb-btn" disabled title={t("toolbar.disabledHint")}>
        {t("toolbar.new")} <span className="tb-tag">{t("toolbar.newPlaceholder")}</span>
      </button>
      <span className="tb-sep" />
      <button className="tb-btn" disabled title={t("toolbar.disabledHint")}>
        {t("toolbar.cut")}
      </button>
      <button className="tb-btn" disabled title={t("toolbar.disabledHint")}>
        {t("toolbar.copy")}
      </button>
      <button className="tb-btn" disabled title={t("toolbar.disabledHint")}>
        {t("toolbar.paste")}
      </button>
      <button className="tb-btn" disabled title={t("toolbar.disabledHint")}>
        {t("toolbar.delete")}
      </button>
      <span className="tb-spring" />
      <button className="tb-btn" disabled title={t("toolbar.disabledHint")}>
        {t("toolbar.sort")} ▾
      </button>
      <button className="tb-btn" disabled title={t("toolbar.disabledHint")}>
        {t("toolbar.view")} ▾
      </button>
    </div>
  );
}
