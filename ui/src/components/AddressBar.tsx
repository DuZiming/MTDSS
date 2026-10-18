/// 地址栏:←/→/↑ 导航按钮(M3a disabled)+ 当前视图面包屑 + 搜索占位。
/// 顶层视图 = 设备列表视图,面包屑即「MTDSS设备」。
import { useTranslation } from "react-i18next";

export default function AddressBar({ breadcrumb }: { breadcrumb: string }) {
  const { t } = useTranslation();
  return (
    <div className="addressbar">
      <button className="nav-btn" disabled title={t("address.back")}>
        ←
      </button>
      <button className="nav-btn" disabled title={t("address.forward")}>
        →
      </button>
      <button className="nav-btn" disabled title={t("address.up")}>
        ↑
      </button>
      <div className="breadcrumb">{breadcrumb}</div>
      <input
        className="search-box"
        type="search"
        disabled
        placeholder={t("address.searchPlaceholder")}
      />
    </div>
  );
}
