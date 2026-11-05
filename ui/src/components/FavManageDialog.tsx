/// 常用目录管理屏(V3-3 Task 13):成员清单(标志色点 + 设备名 + 路径 +
/// 逐行「移除」钮)+ 底部「删除常用目录」danger 钮。重命名不在本弹层
/// (侧栏右键菜单 → FavNameDialog 承担);移除/删除动作回调全在调用方
/// (store op + toast + 视图对账),本组件零业务状态(受控,选择权在
/// App)。设备不在快照 → 行名切「设备已移除」灰标注(设备先于常用被删
/// 的兜底面;色点随 colors 缺失自然不渲染)。busy 在途全部按钮禁用
/// (共享 busy,跨 op 互斥由 store mutate 门保证)。模态沿 FavNameDialog
/// 同款(遮罩+卡片+焦点陷阱;Escape 与遮罩点击均为关闭;open=false 不
/// 渲染)。
import { useEffect } from "react";
import { useTranslation } from "react-i18next";

import type { DeviceInfo } from "../lib/derived";
import type { FavMember, Favorite } from "../lib/favorites";
import { useFocusTrap } from "../lib/useFocusTrap";

export default function FavManageDialog({
  open,
  fav,
  devices,
  colors,
  busy,
  onRemoveMember,
  onDelete,
  onClose,
}: {
  open: boolean;
  /// 管理目标常用(App 从 store 现值解析传入;行序 = 成员序 = 添加序)。
  fav: Favorite;
  devices: DeviceInfo[];
  /// 设备 → 标志色(App 派生的 favColors;无色设备行不渲染色点)。
  colors: Map<string, string>;
  busy: boolean;
  onRemoveMember(member: FavMember): void;
  onDelete(): void;
  onClose(): void;
}) {
  const { t } = useTranslation();
  const ref = useFocusTrap(open);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;
  const deviceName = (deviceId: string): string =>
    devices.find((d) => d.device_id === deviceId)?.name ?? deviceId;
  return (
    <div className="dialog-mask" role="presentation" onClick={onClose}>
      <div
        ref={ref}
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-label={t("favorites.manageTitle")}
        onClick={(e) => e.stopPropagation()}
      >
        <p className="dialog-title">{t("favorites.manageTitle")}</p>
        <ul className="fav-manage-list">
          {fav.members.map((m) => {
            const dot = colors.get(m.device);
            const gone = !devices.some((d) => d.device_id === m.device);
            return (
              <li key={m.device + "\u0000" + m.path} className="fav-manage-row">
                {dot != null && <span className="src-dot fav-member-dot" style={{ background: dot }} />}
                <span className={"fav-manage-name" + (gone ? " removed" : "")}>
                  {gone ? t("favorites.deviceRemoved") : deviceName(m.device)}
                </span>
                <span className="fav-member-path">{m.path}</span>
                <button
                  className="btn fav-manage-remove"
                  disabled={busy}
                  onClick={() => {
                    if (busy) return;
                    onRemoveMember(m);
                  }}
                >
                  {t("favorites.removeMember")}
                </button>
              </li>
            );
          })}
        </ul>
        <div className="dialog-actions">
          <button className="btn danger" disabled={busy} onClick={onDelete}>
            {t("favorites.manageDelete")}
          </button>
        </div>
      </div>
    </div>
  );
}
