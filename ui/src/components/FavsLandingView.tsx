/// 快捷目录着陆页(V3-3 返工裁定五 §17.1):点「快捷目录」组头行与点
/// 「Synshu 设备」组头行同构——本页列出收件箱置顶首行(单击进本机收件
/// 目录浏览,与树行/下拉同一落点;§17.2 成员语义对齐)+ 全部快捷目录行
/// (文件夹图标 + 名 + 成员数副文;双击/回车进该快捷分组视图)。视觉
/// 语言复用 DeviceTable 的详细信息表(.device-table):行 hover/可进、
/// 空态居中引导;在线点/平台列不适用(快捷目录无在线语义,色标 §16.2
/// 已全量移除)。空清单给引导行「左上角「新建」创建快捷目录」(「新建」
/// = 工具栏左上下拉,返工 §13.2 承接)。
import { useTranslation } from "react-i18next";

import type { Favorite } from "../lib/favorites";

import { IcFolder } from "./icons";

export default function FavsLandingView({
  favorites,
  inboxAvailable,
  onOpenInbox,
  onOpenFav,
}: {
  favorites: Favorite[];
  /// 本机收件目录可用(self 设备 id 与 recv_dir 均在场);缺 → 不列
  /// 收件箱行(着陆页仍列快捷目录)。
  inboxAvailable: boolean;
  /// 收件箱行点击/回车:进本机收件目录浏览(App openInbox 同一入口)。
  onOpenInbox(): void;
  /// 快捷目录行双击/回车:进该快捷分组视图(fav 位)。
  onOpenFav(id: string): void;
}) {
  const { t } = useTranslation();
  return (
    <div className="device-table-wrap">
      <table className="device-table favs-table">
        <thead>
          <tr>
            <th>{t("devices.column.name")}</th>
          </tr>
        </thead>
        <tbody>
          {/* 置顶收件箱行(§17.2):单击/回车即进本机收件目录浏览 */}
          {inboxAvailable && (
            <tr
              className="enterable"
              tabIndex={0}
              onClick={onOpenInbox}
              onKeyDown={(e) => {
                if (e.key === "Enter") onOpenInbox();
              }}
            >
              <td className="cell-name">
                <IcFolder size={16} />
                <span className="cell-text">
                  <span className="cell-title">{t("sidebar.inbox")}</span>
                </span>
              </td>
            </tr>
          )}
          {/* 快捷目录行:双击/回车进分组视图(设备表双击进入同款交互语言) */}
          {favorites.map((f) => (
            <tr
              key={f.id}
              className="enterable"
              tabIndex={0}
              onDoubleClick={() => onOpenFav(f.id)}
              onKeyDown={(e) => {
                if (e.key === "Enter") onOpenFav(f.id);
              }}
            >
              <td className="cell-name">
                <IcFolder size={16} />
                <span className="cell-text">
                  <span className="cell-title">{f.name}</span>
                  <span className="cell-sub">
                    {t("favorites.memberCount", { count: f.members.length })}
                  </span>
                </span>
              </td>
            </tr>
          ))}
          {favorites.length === 0 && (
            <tr>
              <td className="empty">{t("favorites.landingEmpty")}</td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}
