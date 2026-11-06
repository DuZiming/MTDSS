/// Sidebar SSR 冒烟(V3-3 返工二 §14.1 树形=成员内容合并,推翻 §13.1
/// 成员目录节点形态;沿 FavView .test.tsx 手法:react-dom/server 纯字符串
/// 渲染,不跑 effects)。引擎侧 useFavListing 渲染期只做 seedSnapshot 纯
/// 分桶:在线成员 pending(加载行)、离线成员 offline(条目缺席、不发
/// 请求,daemonList 断言零调用)——合并行本体由呈现层 FavTreeRows 直驱
/// 钉死:①两成员条目并集在场(目录行可点可落、文件行仅展示不响应)、
/// 来源色点逐行渲染;②离线注记行「N 个成员离线」;③加载行渐进可见;
/// ④「+」新建钮已撤(返工 §13.2);⑤设备树形状不变(组标头在、设备行
/// 收合不渲染);⑥showFavorites=false 整节不渲染(未登录零回归);⑦零
/// 成员快捷展开引导占位行(R4 披露③,R8:favorites.emptyHint)。
/// caret 切换/懒加载/引擎扇出等交互非 SSR 可测(引擎行为由 favfetch.test
/// 直驱钉死、投影排序由 favtree.test 钉死),不在本文件覆盖。
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
import type { MixedRow } from "../lib/mixedview";
import type { Loc } from "../lib/nav";
import Sidebar, { FavTreeRows } from "./Sidebar";

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
/// 已随「+」撤出,返工二 §14.1 起 colors 下发);各例按需覆写。
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
  colors: new Map<string, string>([
    ["dev-on", "#158cd5"],
    ["dev-off", "#d52c15"],
  ]),
  onOpenFav: noop,
  onOpenFavDir: noop,
  onOpenInbox: noop,
  onFavContextMenu: noop,
  onNavigate: noop,
});

/// 合并行构造器(§14.1 呈现层直驱;MixedRow 与 mixedview 同形)。
const rowOf = (device: string, name: string, path: string, isDir: boolean): MixedRow => ({
  entry: { name, path, size: isDir ? 0 : 5, mtime_ms: 0, is_dir: isDir },
  device,
  key: `${device}\u0000${path}`,
});

/// FavTreeRows 直驱全量 props(两成员条目并集:目录+文件各二)。
const treeRowsProps = () => ({
  favId: "f-1",
  rows: [
    rowOf("dev-on", "设计稿", "C:\\P\\设计稿", true),
    rowOf("dev-off", "数据", "/srv/数据", true),
    rowOf("dev-on", "readme.txt", "C:\\P\\readme.txt", false),
    rowOf("dev-off", "总结.docx", "/srv/总结.docx", false),
  ],
  loading: false,
  absent: 0,
  empty: false,
  depth: 2,
  colors: new Map<string, string>([
    ["dev-on", "#158cd5"],
    ["dev-off", "#d52c15"],
  ]),
  deviceById: new Map([
    ["dev-on", devOf("dev-on", "工位机", true)],
    ["dev-off", devOf("dev-off", "仓库机", false)],
  ]),
  current: { kind: "devices" } as Loc,
  expandedKeys: new Set<string>(),
  cache: new Map(),
  dropHoverKey: null,
  onToggleDir: noop,
  onOpenFavDir: noop,
});

describe("FavTreeRows SSR(§14.1 成员内容合并行)", () => {
  it("两成员条目并集在场:目录行可点可落、文件行仅展示不响应,色点逐行渲染", () => {
    const html = renderToStaticMarkup(createElement(FavTreeRows, treeRowsProps()));
    // 两来源条目都在(§14.1 并集;同路径跨源并存由 favtree.test 钉)
    expect(html).toContain("设计稿");
    expect(html).toContain("数据");
    expect(html).toContain("readme.txt");
    expect(html).toContain("总结.docx");
    // 目录恒前:目录标签先于文件标签(跨源同纪律)
    expect(html.indexOf("设计稿")).toBeLessThan(html.indexOf("readme.txt"));
    expect(html.indexOf("数据")).toBeLessThan(html.indexOf("总结.docx"));
    // 来源色点逐行渲染(§4 色板取色;4 行 4 点)
    expect(html.match(/src-dot/g)?.length).toBe(4);
    expect(html).toContain("background:#158cd5");
    expect(html).toContain("background:#d52c15");
    // 目录行交互面:role=button + 拖放落点 device\0path(与设备目录子节点同形)
    expect(html).toContain(`data-drop-loc="dev-on\u0000C:\\P\\设计稿"`);
    expect(html).toContain('role="button"');
    // 文件行非交互标记:tree-file 类 + 无 role/tabindex/落点
    const fileRow = html.match(/<div class="tree-node tree-file"[^>]*>/);
    expect(fileRow).not.toBeNull();
    expect(fileRow![0]).not.toContain("role=");
    expect(fileRow![0]).not.toContain("tabindex");
    expect(fileRow![0]).not.toContain("data-drop-loc");
    expect(html).toContain("tree-file");
  });

  it("离线注记行:absent>0 且加载已定 →「N 个成员离线」;加载中只给加载行", () => {
    const settled = renderToStaticMarkup(createElement(FavTreeRows, { ...treeRowsProps(), absent: 1 }));
    expect(settled).toContain("1 个成员离线");
    expect(settled).toContain("tree-status");
    // 加载中:加载行在场、注记行不抢跑(行渐进出现,定局后才注记)
    const loading = renderToStaticMarkup(createElement(FavTreeRows, { ...treeRowsProps(), loading: true, absent: 1 }));
    expect(loading).toContain("加载中");
    expect(loading).not.toContain("个成员离线");
  });

  it("空行集 + 空快捷标记:只给引导占位行(favorites.emptyHint)", () => {
    const html = renderToStaticMarkup(
      createElement(FavTreeRows, { ...treeRowsProps(), rows: [], empty: true }),
    );
    expect(html).toContain("到设备浏览中添加目录");
    expect(html).toContain("tree-status");
    expect(html).not.toContain("src-dot");
  });
});

describe("Sidebar SSR 冒烟(V3-3 返工二 §14.1:树形=成员内容合并)", () => {
  it("组默认展开:组标头/收件箱/快捷行在场,成员目录节点形态已撤,在线成员给加载行,零拉取", () => {
    const html = renderToStaticMarkup(createElement(Sidebar, baseProps()));
    // 组标头(返工 §13.9 侧栏部分)与组开态 caret
    expect(html).toContain("快捷目录");
    expect(html).toContain("caret open");
    // 置顶收件箱行仍在;快捷行词面不变
    expect(html).toContain("收件箱");
    expect(html).toContain(">项目</span>");
    // §13.1 成员目录节点形态已撤(标签=路径末段 / title=设备名·全路径 不再出现)
    expect(html).not.toContain("项目文档");
    expect(html).not.toContain('title="工位机 · C:\\Users\\me\\项目文档"');
    // 在线成员 pending → 合并子级加载行渐进可见(渲染期只 seed,不发请求)
    expect(html).toContain("加载中");
    // 在线成员条目未到 → 无 device\0path 落点(懒加载行展开后才挂)
    expect(html).not.toContain(`data-drop-loc="dev-on\u0000C:\\Users\\me\\项目文档"`);
    // 快捷行拖放落点键不变(T12 契约 fav\0id)
    expect(html).toContain(`data-drop-loc="fav\u0000f-1"`);
    // 渲染期零拉取(effects 不跑;离线成员引擎种子即 offline 桶从不发)
    expect(listMock).not.toHaveBeenCalled();
  });

  it("全离线成员:条目缺席、无加载行、尾部注记「N 个成员离线」", () => {
    const html = renderToStaticMarkup(
      createElement(Sidebar, {
        ...baseProps(),
        devices: [devOf("dev-off", "仓库机", false), devOf("dev-off2", "库房机", false)],
        favorites: [
          favOf("f-1", "项目", [
            { device: "dev-off", path: "/srv/数据" },
            { device: "dev-off2", path: "/mnt/仓" },
          ]),
        ],
      }),
    );
    expect(html).toContain("项目");
    expect(html).not.toContain("加载中");
    expect(html).toContain("2 个成员离线");
    expect(html).toContain("tree-status");
    // 合并行零在场 → 无目录行落点(device\0path 形态不出现)
    expect(html).not.toContain(`data-drop-loc="dev-off\u0000`);
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
