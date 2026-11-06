/// Sidebar SSR 冒烟(V3-3 返工 R4,spec §13.1 快捷目录树形化;沿 FavView
/// .test.tsx 手法:react-dom/server 纯字符串渲染,不跑 effects,渲染期不发
/// 任何请求——成员子树默认收合在 DirNode 层,daemonList 断言零调用)。钉死
/// 词面/结构:①快捷目录组默认展开→组标头「快捷目录」+置顶收件箱行+快捷行
/// +成员目录节点(标签=路径末段,title=设备名 · 全路径)静态可见,离线成员
/// 灰显禁点;②「+」新建钮已撤(返工 §13.2);③设备树形状不变(组标头在、
/// 设备行收合不渲染);④showFavorites=false 整节不渲染(未登录零回归);
/// ⑤零成员快捷行展开给引导占位行(R4 披露③,R8:favorites.emptyHint)。
/// caret 切换/懒加载等交互非 SSR 可测,不在本文件覆盖(纯结构断言)。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

const listMock = vi.fn();
vi.mock("../lib/daemon", () => ({
  daemonList: (...args: unknown[]) => listMock(...args),
}));

import "../i18n";

import type { DeviceInfo } from "../lib/derived";
import type { Favorite } from "../lib/favorites";
import type { Loc } from "../lib/nav";
import Sidebar from "./Sidebar";

const devOf = (id: string, name: string, online: boolean): DeviceInfo => ({
  device_id: id,
  name,
  platform: "windows",
  online,
});

const favOf = (
  id: string,
  name: string,
  members: { device: string; path: string }[],
): Favorite => ({ id, name, created_at: 1, members });

const noop = () => {};

/// 全量 props(与 App.tsx Sidebar 位下发同形;R4 起 onNewFav/favReadOnly
/// 已随「+」撤出);各例按需覆写。
const baseProps = () => ({
  devices: [devOf("dev-on", "工位机", true), devOf("dev-off", "仓库机", false)],
  selfDeviceId: "dev-on",
  current: { kind: "devices" } as Loc,
  dropHoverKey: null,
  favorites: [
    favOf("f-1", "项目", [
      { device: "dev-on", path: "C:\\Users\\me\\项目文档" },
      { device: "dev-off", path: "/srv/数据" },
    ]),
    favOf("f-2", "空快捷", []),
  ],
  inboxPath: "C:\\Users\\me\\Documents\\MTDSS",
  showFavorites: true,
  onOpenFav: noop,
  onOpenFavDir: noop,
  onOpenInbox: noop,
  onFavContextMenu: noop,
  onNavigate: noop,
});

describe("Sidebar SSR 冒烟(V3-3 返工 R4:快捷目录树形化)", () => {
  it("组默认展开:组标头/收件箱/快捷行/成员目录节点静态可见,离线成员灰显,零拉取", () => {
    const html = renderToStaticMarkup(createElement(Sidebar, baseProps()));
    // 组标头改名「快捷目录」(返工 §13.9 侧栏部分),组开态 caret
    expect(html).toContain("快捷目录");
    expect(html).toContain("caret open");
    // 置顶收件箱行仍在
    expect(html).toContain("收件箱");
    // 快捷行 + 成员目录节点(默认展开):标签=路径末段
    expect(html).toContain(">项目</span>");
    expect(html).toContain("项目文档");
    expect(html).toContain("数据");
    // 成员行 title=设备名 · 全路径(来源弱化进悬浮提示)
    expect(html).toContain('title="工位机 · C:\\Users\\me\\项目文档"');
    expect(html).toContain('title="仓库机 · /srv/数据"');
    // 离线成员 DirNode 灰显禁点(快照 join online=false)
    expect(html).toContain("tree-node disabled");
    // 快捷行拖放落点键不变(T12 契约 fav\0id)
    expect(html).toContain(`data-drop-loc="fav\u0000f-1"`);
    // 在线成员目录节点落点键=device\0path(与设备目录子节点同形)
    expect(html).toContain(`data-drop-loc="dev-on\u0000C:\\Users\\me\\项目文档"`);
    // 渲染期零拉取(成员子树默认收合在 DirNode 层,懒加载只在展开时发起)
    expect(listMock).not.toHaveBeenCalled();
  });

  it("「+」新建钮已撤(返工 §13.2),R5 前侧栏无新建入口", () => {
    const html = renderToStaticMarkup(createElement(Sidebar, baseProps()));
    expect(html).not.toContain("fav-add");
    expect(html).not.toContain("<button");
  });

  it("零成员快捷行展开给引导占位(R4 披露③,R8):emptyHint 行在场", () => {
    const html = renderToStaticMarkup(createElement(Sidebar, baseProps()));
    // f-2「空快捷」0 成员:缺省展开态下渲染引导行(「到设备浏览中添加目录」,
    // 复用 favorites.emptyHint,与混合视图空态同词面;tree-status 同空目录形)
    expect(html).toContain("空快捷");
    expect(html).toContain("到设备浏览中添加目录");
    expect(html).toContain("tree-status");
  });

  it("设备树形状不变:「Synshu 设备」组标头在,默认收合设备行不渲染", () => {
    const html = renderToStaticMarkup(createElement(Sidebar, baseProps()));
    expect(html).toContain("Synshu 设备");
    // 设备组默认收合(devicesOpen=false):设备行(本机后缀)不出现
    expect(html).not.toContain("(本机)");
  });

  it("showFavorites=false:快捷整节不渲染(未登录零回归),设备组仍在", () => {
    const html = renderToStaticMarkup(
      createElement(Sidebar, {
        ...baseProps(),
        showFavorites: false,
        favorites: [],
        inboxPath: null,
      }),
    );
    expect(html).not.toContain("快捷目录");
    expect(html).not.toContain("收件箱");
    expect(html).not.toContain(`fav\u0000`);
    expect(html).toContain("Synshu 设备");
    expect(listMock).not.toHaveBeenCalled();
  });
});
