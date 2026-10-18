/// 状态栏:信令:{未登录|离线|在线} · {N} 台设备。
import { useTranslation } from "react-i18next";

import { onlineText, type LoginStatus } from "../lib/derived";

export default function StatusBar({
  status,
  deviceCount,
}: {
  status: LoginStatus | null;
  deviceCount: number;
}) {
  const { t } = useTranslation();
  const text = status == null ? "…" : onlineText(status);
  return (
    <div className="statusbar">
      <span>
        {t("status.signaling")}:{text}
      </span>
      <span className="status-sep">·</span>
      <span>{t("status.deviceCount", { count: deviceCount })}</span>
    </div>
  );
}
