/// 状态栏:目录视图 = 「{n} 个项目[,已选 m]」;设备视图 = 现行行为。
/// 尾部恒为 信令:{未登录|离线|在线} · {N} 台设备。
import { useTranslation } from "react-i18next";

import { onlineText, type LoginStatus } from "../lib/derived";

export default function StatusBar({
  status,
  deviceCount,
  itemCount,
  selectedCount,
}: {
  status: LoginStatus | null;
  deviceCount: number;
  /// 目录视图条目数;null = 设备视图(不展示条目计数)。
  itemCount: number | null;
  selectedCount: number;
}) {
  const { t } = useTranslation();
  const text = status == null ? "…" : onlineText(status);
  const browsing = itemCount != null;
  return (
    <div className="statusbar">
      {browsing && (
        <>
          <span>
            {t("status.items", { count: itemCount ?? 0 })}
            {selectedCount > 0 ? t("status.selectedSuffix", { count: selectedCount }) : ""}
          </span>
          <span className="status-sep">·</span>
        </>
      )}
      <span>
        {t("status.signaling")}:{text}
      </span>
      <span className="status-sep">·</span>
      <span>{t("status.deviceCount", { count: deviceCount })}</span>
    </div>
  );
}
