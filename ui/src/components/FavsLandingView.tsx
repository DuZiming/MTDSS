/// 快捷目录着陆页(V3-3 返工裁定五 §17.1;行形返工裁定六 §18.2):点
/// 「快捷目录」组头行与点「Synshu 设备」组头行同构——本页列出收件箱置顶
/// 首行(单击进本机收件目录浏览,与树行/下拉同一落点;§17.2 成员语义对
/// 齐)+ 全部快捷目录行(文件夹图标 + 名,单行与 DeviceTable 行同高同
/// 形——成员数副文 §18.2 裁定撤除)。视觉语言复用 DeviceTable 的详细信
/// 息表(.device-table):行 hover/可进、空态居中引导;在线点/平台列不适
/// 用(快捷目录无在线语义,色标 §16.2 已全量移除)。空清单给引导行「左
/// 上角「新建」创建快捷目录」(「新建」= 工具栏左上下拉,返工 §13.2 承
/// 接)。
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
      <table className="device-table">
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
                <span>{t("sidebar.inbox")}</span>
              </td>
            </tr>
          )}
          {/* 快捷目录行:双击/回车进分组视图(设备表双击进入同款交互语言);
              单行名(§18.2 同形,无副文) */}
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
                <span>{f.name}</span>
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
