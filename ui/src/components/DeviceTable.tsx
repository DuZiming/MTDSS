/// 设备列表(详细信息式表格):名称 / 平台 / 在线。
/// 本机行置顶且名称后缀「(本机)」;离线行灰显。
import { useTranslation } from "react-i18next";

import { sortDevices, type DeviceInfo } from "../lib/derived";

export default function DeviceTable({
  devices,
  selfDeviceId,
}: {
  devices: DeviceInfo[];
  selfDeviceId: string | null;
}) {
  const { t } = useTranslation();
  const sorted = sortDevices(devices, selfDeviceId);
  return (
    <div className="device-table-wrap">
      <table className="device-table">
        <thead>
          <tr>
            <th>{t("devices.column.name")}</th>
            <th>{t("devices.column.platform")}</th>
            <th>{t("devices.column.online")}</th>
          </tr>
        </thead>
        <tbody>
          {sorted.map((d) => {
            const isSelf = selfDeviceId != null && d.device_id === selfDeviceId;
            return (
              <tr key={d.device_id} className={d.online ? "" : "offline"}>
                <td className="cell-name">
                  {d.name ?? d.device_id}
                  {isSelf ? `(${t("devices.self")})` : ""}
                </td>
                <td>{d.platform}</td>
                <td>
                  <span className={d.online ? "dot on" : "dot off"}>●</span>{" "}
                  {d.online ? t("devices.online") : t("devices.offline")}
                </td>
              </tr>
            );
          })}
          {sorted.length === 0 && (
            <tr>
              <td colSpan={3} className="empty">
                {t("devices.empty")}
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}
