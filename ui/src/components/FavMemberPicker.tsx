/// 「存入哪个成员」选择层(V3-3 Task 12):拖放/粘贴进多成员常用目录时
/// 解析落点——拖放落点键 `fav\u0000<id>` 与 fav 位粘贴解析出 members>1
/// 即开本层,行 = 成员标志色点 + 设备名 + 路径,点选即传(onPick 交给
/// App 的同一 fan-out/粘贴管线)。模态沿 FavPickerDialog 同款(遮罩+卡片
/// +焦点陷阱;Escape 与遮罩点击均为关闭;点选即时无 busy 位)。离线成员
/// 行禁选(brief Step 1 裁定:路由到远端上传由引擎拒,UI 不预判——简化
/// 为离线禁选,title 带不可选提示)。清单样式沿 .fav-picker-list,行内
/// 路径右置弱化(.fav-member-path)。
import { useEffect } from "react";
import { useTranslation } from "react-i18next";

import type { DeviceInfo } from "../lib/derived";
import type { FavMember, Favorite } from "../lib/favorites";
import { useFocusTrap } from "../lib/useFocusTrap";

export default function FavMemberPicker({
  open,
  fav,
  devices,
  colors,
  onPick,
  onClose,
}: {
  open: boolean;
  /// 待解析的常用条目(成员序 = 添加序 = 行序)。
  fav: Favorite;
  devices: DeviceInfo[];
  /// 设备 → 标志色(App 派生的 favColors;无色设备行不渲染色点)。
  colors: Map<string, string>;
  onPick(member: FavMember): void;
  onClose(): void;
}) {
  const { t } = useTranslation();
  const ref = useFocusTrap(open);
  const deviceName = (deviceId: string): string =>
    devices.find((d) => d.device_id === deviceId)?.name ?? deviceId;
  const offline = (deviceId: string): boolean =>
    !devices.some((d) => d.device_id === deviceId && d.online);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;
  return (
    <div className="dialog-mask" role="presentation" onClick={onClose}>
      <div
        ref={ref}
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-label={t("favorites.pickMemberTitle")}
        onClick={(e) => e.stopPropagation()}
      >
        <p className="dialog-title">{t("favorites.pickMemberTitle")}</p>
        <ul className="fav-picker-list">
          {fav.members.map((m) => {
            const dot = colors.get(m.device);
            const off = offline(m.device);
            return (
              <li key={m.device + "\u0000" + m.path}>
                <button
                  className="fav-picker-item"
                  disabled={off}
                  title={off ? t("favorites.memberOfflineRow") : undefined}
                  onClick={() => {
                    if (off) return;
                    onPick(m);
                  }}
                >
                  {dot != null && <span className="src-dot fav-member-dot" style={{ background: dot }} />}
                  <span className="fav-picker-name">{deviceName(m.device)}</span>
                  <span className="fav-member-path">{m.path}</span>
                </button>
              </li>
            );
          })}
        </ul>
        <div className="dialog-actions">
          <button className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
        </div>
      </div>
    </div>
  );
}
