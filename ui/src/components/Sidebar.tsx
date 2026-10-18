/// 左侧树:「常用目录」灰占位(V2)+「MTDSS设备」可点击节点。
/// 点击 MTDSS设备 → 主区域显示设备列表(设计 §3:与地址栏顶层视图同效)。
/// M3a 的树为简单列表,展开箭头仅示意(设备子节点 V2 逐级展开)。
import { useTranslation } from "react-i18next";

export default function Sidebar({
  devicesSelected,
  onSelectDevices,
}: {
  devicesSelected: boolean;
  onSelectDevices: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="sidebar">
      <div className="tree-node disabled" title={t("sidebar.favoritesTag")}>
        <span className="caret">▸</span>
        <span className="tree-label">{t("sidebar.favorites")}</span>
        <span className="tree-tag">{t("sidebar.favoritesTag")}</span>
      </div>
      <div
        className={
          "tree-node clickable" + (devicesSelected ? " selected" : "")
        }
        onClick={onSelectDevices}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") onSelectDevices();
        }}
        role="button"
        tabIndex={0}
      >
        <span className="caret">▾</span>
        <span className="tree-label strong">{t("sidebar.devices")}</span>
      </div>
    </div>
  );
}
