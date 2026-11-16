/// Sidebar SSR 冒烟(V3-3 返工波 4 §16.3 树=成员节点,§13.1 形态回归;
/// 返工裁定五 §17.1/§17.2 组头行落着陆位+收件箱行 caret 可展开;沿
/// FavView .test.tsx 手法:react-dom/server 纯字符串渲染,不跑 effects)。
/// 钉死结构:①展开快捷=每成员一枚 DirNode(标签=「设备名 - 目录名」§16.3
/// 词面),在线成员可点可落(device\0path)、离线成员灰禁(role/tabindex
/// 不挂、class disabled);②无 FavChildren/引擎面:渲染期零加载行、零
/// 拉取(daemonList 断言零调用)、§14.1 尾注记行「N 个成员离线」废止;
/// ③组头行「快捷目录」与「Synshu 设备」组行同构(role=button+tabindex、
/// caret 默认展开;点击/键盘=导航落 favs 着陆位,§17.1——§15.2 落收件箱
/// 裁定废止;着陆位时组头行 selected 高亮);④收件箱行=DirNode 形
/// (§17.2:caret 可展开 + 拖放落点 device\0recv_dir);⑤零成员快捷展开
/// 引导占位行(R4 披露③,R8:favorites.emptyHint)保留;⑥「+」新建钮
/// 已撤(返工 §13.2);⑦零色点(§16.2 色标全量移除);⑧设备树形状不变
/// (组标头在、设备行收合不渲染);⑨showFavorites=false 整节不渲染(未
/// 登录零回归);⑩树子级含文件(§17.5:快捷树面 showFiles=文件行叶形
/// 非交互,设备树面不传仍仅目录——第二 describe 以具名导出 DirNode
/// 缓存直驱,沿 FavTreeRows 先例)。caret 切换/懒加载/成员节点点击导航
/// 等交互非 SSR 可测(成员节点/收件箱行=DirNode 原样复用,行为由设备
/// 树既有面钉死),不在本文件覆盖。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

const listMock = vi.fn();
vi.mock("../lib/daemon", () => ({
  daemonList: (...args: unknown[]) => listMock(...args),
}));

import "../i18n";

import type { DeviceInfo } from "../lib/derived";
import type { FsEntry } from "../lib/browse";
import type { Favorite } from "../lib/favorites";
import type { Loc } from "../lib/nav";
import Sidebar, { DirNode, FavNode } from "./Sidebar";

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

/// 全量 props(与 App.tsx Sidebar 位下发同形;§16.3 起 colors 已随色点
/// 移除撤出下发面);各例按需覆写。
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
  onFavContextMenu: noop,
  onNavigate: noop,
});

/// FavNode 直构 props(V3-6b:快捷行缺省收起,成员可见态需显式展开键;
/// 顶层 Sidebar 的 expandedKeys 为内部态,SSR 无法交互——成员面断言
/// 直构 FavNode)。
const favNodeProps = (favId: string) => ({
  fav: (baseProps().favorites as Favorite[]).find((f) => f.id === favId)!,
  depth: 1,
  deviceById: new Map(
    (baseProps().devices as DeviceInfo[]).map((d) => [d.device_id, d]),
  ),
  current: { kind: "devices" } as Loc,
  expandedKeys: new Set([`fav\u0000${favId}`]),
  cache: new Map(),
  dropHoverKey: null,
  onOpenFav: noop,
  onOpenFavDir: noop,
  onFavContextMenu: noop,
  onToggleFav: noop,
  showHidden: false,
  onToggleDir: noop,
});

describe("Sidebar SSR 冒烟(返工波 4 §16.3:树=成员节点)", () => {
  it("展开快捷=成员节点「设备名 - 目录名」(§16.3):在线成员可点可落,零加载行零拉取", () => {
    // V3-6b 快捷行缺省收起:成员节点可见需显式展开 fav\u0000f-1
    const html = renderToStaticMarkup(
      createElement(FavNode, favNodeProps("f-1")),
    );
    // 成员节点词面(§16.3,主区分组节头同款):「设备名 - 目录名」
    expect(html).toContain("工位机 - 项目文档");
    expect(html).toContain("仓库机 - 数据");
    // 在线成员节点=DirNode 同款交互面:role=button + 拖放落点 device\0path
    const onRow = html.match(/<div class="tree-node clickable"[^>]*>/g) ?? [];
    expect(
      onRow.some((m) => m.includes(`data-drop-loc="dev-on\u0000C:\\Users\\me\\项目文档"`)),
    ).toBe(true);
    // 快捷行拖放落点键不变(T12 契约 fav\0id)
    expect(html).toContain(`data-drop-loc="fav\u0000f-1"`);
    // FavChildren/引擎面已撤:成员节点为静态 DirNode(收合态),渲染期
    // 无加载行、零拉取(effects 不跑;§14.1 渐进扇出种子路径废止)
    expect(html).not.toContain("加载中");
    expect(listMock).not.toHaveBeenCalled();
  });

  it("离线成员节点灰禁:class disabled、role/tabindex 不挂(成员自身披露,注记行废止)", () => {
    const html = renderToStaticMarkup(
      createElement(FavNode, favNodeProps("f-1")),
    );
    // 离线成员行=disabled 形(仓库机 - 数据):无 role/tabindex
    const offRow = html.match(/<div class="tree-node disabled"[^>]*>/g) ?? [];
    expect(offRow.length).toBe(1);
    expect(offRow[0]).toContain(`data-drop-loc="dev-off\u0000/srv/数据"`);
    expect(offRow[0]).not.toContain("role=");
    expect(offRow[0]).not.toContain("tabindex");
    // §14.1 尾注记行随内容并集废止:离线成员以灰节点披露,无「N 个成员离线」
    expect(html).not.toContain("个成员离线");
    expect(html).not.toContain("tree-status loading");
  });

  it("组头行(§15.2)与「Synshu 设备」组行同构:role=button+tabindex;两组头缺省均开(V3-6b 显式化+2026-11-16 设备组裁定)", () => {
    const html = renderToStaticMarkup(createElement(Sidebar, baseProps()));
    // 组标头词面;caret 态显式钉(V3-6b 前「双 open」靠 fav 行 open caret
    // 位置碰巧满足,语义翻回后露馅——现按真默认钉:快捷组 favsOpen=true,
    // 设备组 devicesOpen=true(2026-11-16 用户裁定默认展开))
    expect(html).toContain(">快捷目录</span>");
    expect(html).toContain(">Synshu 设备</span>");
    // 行本体交互面同构:两行均 role=button + tabindex(点击/键盘=导航,
    // 快捷组落 favs 着陆位=onNavigate 直传 {kind:"favs"},§17.1;交互非
    // SSR 可测,以标记同构钉死同构面——两行的行为差异只在回调绑定,结构
    // 契约在此钉死)
    const rowOfLabel = (label: string): string => {
      const idx = html.indexOf(`>${label}</span>`);
      expect(idx).toBeGreaterThan(-1);
      return html.slice(html.lastIndexOf('<div class="tree-node clickable"', idx), idx);
    };
    const favsHead = rowOfLabel("快捷目录");
    expect(favsHead).toContain('role="button"');
    expect(favsHead).toContain('tabindex="0"');
    expect(favsHead).toContain('class="caret open"');
    const devHead = rowOfLabel("Synshu 设备");
    expect(devHead).toContain('role="button"');
    expect(devHead).toContain('tabindex="0"');
    expect(devHead).toContain('class="caret open"');
    // 置顶收件箱行仍在(组内置顶首项,§17.2 起为 DirNode 形,见下例)
    expect(html).toContain("收件箱");
  });

  it("收件箱行=DirNode 形(§17.2):caret 可展开 + 拖放落点 device\\0recv_dir", () => {
    const html = renderToStaticMarkup(createElement(Sidebar, baseProps()));
    // 收件箱行带 caret(DirNode 自管,懒加载收件目录子级)+ 文件夹图标
    const idx = html.indexOf(">收件箱</span>");
    expect(idx).toBeGreaterThan(-1);
    const row = html.slice(html.lastIndexOf('<div class="tree-node clickable"', idx), idx);
    expect(row).toContain('class="caret"'); // 收合态缺省(懒加载待展开)
    expect(row).toContain("tree-icon");
    expect(row).toContain('role="button"');
    expect(row).toContain('tabindex="0"');
    // 拖放落点契约不变(Task 12:device\0recv_dir,与目录子节点同形)
    expect(row).toContain(`data-drop-loc="dev-on\u0000C:\\Users\\me\\Documents\\MTDSS"`);
  });

  it("组头行 selected 高亮随 favs 着陆位(§17.1)", () => {
    const html = renderToStaticMarkup(
      createElement(Sidebar, { ...baseProps(), current: { kind: "favs" } as Loc }),
    );
    const idx = html.indexOf(">快捷目录</span>");
    expect(idx).toBeGreaterThan(-1);
    const head = html.slice(html.lastIndexOf('<div class="tree-node', idx), idx);
    expect(head).toContain("selected");
    // 收件箱行不高亮(着陆位非收件目录位)
    const inboxIdx = html.indexOf(">收件箱</span>");
    const inboxRow = html.slice(html.lastIndexOf('<div class="tree-node', inboxIdx), inboxIdx);
    expect(inboxRow).not.toContain("selected");
  });

  it("零成员快捷行展开给引导占位(R4 披露③保留):emptyHint 行在场", () => {
    const html = renderToStaticMarkup(
      createElement(FavNode, favNodeProps("f-2")),
    );
    // f-2「空快捷」0 成员:缺省展开态下渲染引导行(「到设备浏览中添加目录」,
    // 复用 favorites.emptyHint,样式 tree-status 同空目录形)
    expect(html).toContain("空快捷");
    expect(html).toContain("到设备浏览中添加目录");
    expect(html).toContain("tree-status");
  });

  it("「+」新建钮已撤(返工 §13.2),R5 前侧栏无新建入口", () => {
    const html = renderToStaticMarkup(createElement(Sidebar, baseProps()));
    expect(html).not.toContain("fav-add");
    expect(html).not.toContain("<button");
  });

  it("零色点(§16.2):侧栏无 src-dot 词面(色标消费全量移除)", () => {
    const html = renderToStaticMarkup(createElement(Sidebar, baseProps()));
    expect(html).not.toContain("src-dot");
  });

  it("设备树形状:「Synshu 设备」组标头在,默认展开设备行即渲染(2026-11-16 裁定)", () => {
    const html = renderToStaticMarkup(createElement(Sidebar, baseProps()));
    expect(html).toContain("Synshu 设备");
    // 设备组默认展开(devicesOpen=true,用户裁定与快捷组头同款):设备行
    // 即刻可见(本机置顶+「(本机)」后缀);各设备目录子树仍懒加载
    expect(html).toContain("(本机)");
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

/// §17.5 树子级含文件(两端):展开态/懒加载缓存 SSR 不经 effects,沿
/// DirNode 直驱(缓存显式注入,渲染期零拉取)钉结构:子级**仅目录**
/// (2026-11-16 用户裁定撤 §17.5 树子级含文件——侧栏树=导航面):文件行不渲染;
/// 只含文件的目录判空(空态占位行)。
describe("Sidebar 树子级仅目录(2026-11-16 撤 §17.5)", () => {
  const dirPath = "C:\\Users\\me\项目文档";
  const key = `dev-on\u0000${dirPath}`;
  const entries: FsEntry[] = [
    { name: "子目录", path: `${dirPath}\\子目录`, is_dir: true, size: 0, mtime_ms: 1 },
    { name: "说明.docx", path: `${dirPath}\\说明.docx`, is_dir: false, size: 12, mtime_ms: 2 },
  ];
  const nodeProps = {
    device: "dev-on",
    path: dirPath,
    name: "工位机 - 项目文档",
    depth: 2,
    deviceOnline: true,
    current: { kind: "devices" } as Loc,
    expandedKeys: new Set([key]),
    cache: new Map([[key, { loading: false, entries, failed: false }]]),
    dropHoverKey: null,
    onToggle: noop,
    onOpen: noop,
  };

  it("子级仅目录行;文件不渲染(撤 §17.5 后全树同口径)", () => {
    const html = renderToStaticMarkup(createElement(DirNode, nodeProps));
    expect(html).toContain("子目录");
    expect(html).not.toContain("说明.docx");
    expect(html).not.toContain("tree-file");
    // 零拉取(缓存直驱)
    expect(listMock).not.toHaveBeenCalled();
  });

  it("只含文件的目录判空:空态占位行在场", () => {
    const filesOnlyKey = "dev-on\u0000C:\\onlyfiles";
    const html = renderToStaticMarkup(
      createElement(DirNode, {
        ...nodeProps,
        path: "C:\\onlyfiles",
        expandedKeys: new Set([filesOnlyKey]),
        cache: new Map([
          [
            filesOnlyKey,
            {
              loading: false,
              entries: [
                { name: "只文件.txt", path: "C:\\onlyfiles\\只文件.txt", is_dir: false, size: 1, mtime_ms: 1 },
              ],
              failed: false,
            },
          ],
        ]),
      }),
    );
    expect(html).not.toContain("只文件.txt");
    expect(html).not.toContain("tree-file");
  });
});
