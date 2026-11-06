/// FavView SSR 冒烟(V3-3 Task 9;返工波 4 §16.1 分类恒定重写:混合模式
/// 与 viewOrg/色点消费已删,恒按成员分节。沿 FileList.test.tsx 手法:
/// react-dom/server 纯字符串渲染,不跑 effects;渲染期 useFavListing 只做
/// seedSnapshot 纯分桶——成员全离线时初始即 offline 桶、不发任何请求,
/// daemonList 断言零调用,渲染确定)。钉死词面:①节头 =「设备名 - 目录名」
/// (§16.1);②两成员全离线→两节头带离线标注 + 底部「N 个成员离线或不
/// 可用」注记行;③空快捷(0 成员)→空态引导;④details 单表(<thead=1,
/// §14.4);⑤宿主 data-drop-loc=`fav\0id`(T12 拖放落点契约);⑥全视图
/// 零色点(§16.2 UI 零消费)。
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
import FavView from "./FavView";

const offlineDev = (id: string, name = id): DeviceInfo => ({
  device_id: id,
  name,
  platform: "windows",
  online: false,
});

const favOf = (id: string, members: { device: string; path: string }[]): Favorite => ({
  id,
  name: id,
  created_at: 1,
  members,
});

const noop = () => {};

/// 全量 props(与 App.tsx fav 位下发同形;成员设备全离线 → 渲染期零拉取);
/// 各例按需覆写。
const baseProps = (fav: Favorite) => ({
  fav,
  devices: [offlineDev("dev-1"), offlineDev("dev-2")],
  viewMode: "list" as const,
  sortState: { key: "name" as const, dir: "asc" as const },
  reloadTick: 0,
  selected: new Set<string>(),
  onSelected: noop,
  searchQ: "",
  onOpenDir: noop,
  onRowContextMenu: noop,
  onBlankContextMenu: noop,
  onRetry: noop,
  onSortKey: noop,
  onCount: noop,
  dropHoverKey: null,
});

describe("FavView SSR 冒烟(§16.1 分类恒定)", () => {
  it("节头词面 =「设备名 - 目录名」(§16.1),设备缺快照回退设备 id", () => {
    const fav = favOf("f-head", [{ device: "dev-1", path: "C:\\work" }]);
    const html = renderToStaticMarkup(
      createElement(FavView, {
        ...baseProps(fav),
        devices: [offlineDev("dev-1", "台式机")],
      }),
    );
    expect(html).toContain("<strong>台式机</strong>");
    expect(html).toContain('class="fav-group-path"> - work</span>');
    // 设备不在快照:回退设备 id(现行为)
    const fav2 = favOf("f-head2", [{ device: "ghost", path: "/docs" }]);
    const html2 = renderToStaticMarkup(createElement(FavView, baseProps(fav2)));
    expect(html2).toContain("<strong>ghost</strong>");
    expect(html2).toContain('class="fav-group-path"> - docs</span>');
  });

  it("两成员全离线:两节头(节序=成员添加序)各带离线标注+「 - 目录名」,无行,底部注记仍在", () => {
    const fav = favOf("f-off", [
      { device: "dev-1", path: "C:\\a" },
      { device: "dev-2", path: "/b" },
    ]);
    const html = renderToStaticMarkup(createElement(FavView, baseProps(fav)));
    expect((html.match(/fav-group-head/g) ?? []).length).toBe(2);
    expect((html.match(/fav-group-tag/g) ?? []).length).toBe(2);
    // 节头 caret(返工 §13.5):每节头一枚且默认全展(caret open);
    // SSR 无交互,切换不在此钉(类名契约)
    expect((html.match(/"caret open"/g) ?? []).length).toBe(2);
    expect(html).toContain("离线");
    expect(html).toContain("dev-1");
    expect(html).toContain("dev-2");
    // 节序 = 成员添加序:首节头为 dev-1
    const firstHead = html.slice(html.indexOf("fav-group-head"));
    expect(firstHead.indexOf("dev-1")).toBeLessThan(firstHead.indexOf("dev-2"));
    // 离线节头不挂节级拖放落点;唯一落点 = 宿主 fav\0id(T12 契约不回退)
    expect((html.match(/data-drop-loc/g) ?? []).length).toBe(1);
    expect(html).toContain(`data-drop-loc="fav\u0000f-off"`);
    // 底部不可用注记(T9 裁定保留)
    expect(html).toContain("fav-unavailable");
    expect(html).toContain("2 个成员离线或不可用");
    expect(html).not.toContain("data-row-path"); // 离线节无行
    expect(listMock).not.toHaveBeenCalled(); // 离线成员从不拉取(Review Focus #3)
  });

  it("空快捷(0 成员):空态引导词面,无离线注记", () => {
    const html = renderToStaticMarkup(
      createElement(FavView, baseProps(favOf("f-empty", []))),
    );
    expect(html).toContain("fav-empty");
    expect(html).toContain("到设备浏览中添加目录");
    expect(html).not.toContain("fav-unavailable");
  });

  it("宿主挂拖放落点键 fav\\u0000id(T12 契约)与 view-host 类", () => {
    const html = renderToStaticMarkup(
      createElement(FavView, baseProps(favOf("f-k", []))),
    );
    expect(html).toContain(`data-drop-loc="fav\u0000f-k"`);
    expect(html).toContain("view-host");
  });

  it("在线成员:节头挂节级拖放落点 device\\u0000path+加载中标注,零拉取", () => {
    const fav = favOf("f-on", [{ device: "dev-1", path: "C:\\a" }]);
    const html = renderToStaticMarkup(
      createElement(FavView, {
        ...baseProps(fav),
        devices: [{ ...offlineDev("dev-1"), online: true }],
      }),
    );
    expect(html).toContain(`data-drop-loc="dev-1\u0000C:\\a"`);
    expect((html.match(/"caret open"/g) ?? []).length).toBe(1); // 节头 caret(§13.5)
    expect(html).toContain("加载中…");
    expect(html).not.toContain("fav-unavailable"); // pending 不入不可用注记
    expect(listMock).not.toHaveBeenCalled(); // SSR 不跑 effect,零拉取
  });

  it("详细信息(§14.4):全页表头恰一枚(<thead 计数=1),节头/落点/注记契约不变", () => {
    const fav = favOf("f-det", [
      { device: "dev-1", path: "C:\\a" },
      { device: "dev-2", path: "/b" },
    ]);
    const html = renderToStaticMarkup(
      createElement(FavView, { ...baseProps(fav), viewMode: "details" }),
    );
    // 返工裁定核心断言:单列头(此前每节各带一枚 FileDetails 表头)
    expect((html.match(/<thead/g) ?? []).length).toBe(1);
    // 节头(两节,节序=成员添加序)与灰标注契约原样
    expect((html.match(/fav-group-head/g) ?? []).length).toBe(2);
    expect((html.match(/"caret open"/g) ?? []).length).toBe(2);
    expect(html).toContain("离线");
    // 离线节不挂节级落点;唯一落点 = 宿主 fav\0id(T12 契约不回退)
    expect((html.match(/data-drop-loc/g) ?? []).length).toBe(1);
    expect(html).toContain(`data-drop-loc="fav\u0000f-det"`);
    // 底部不可用注记保留(T9 裁定);无行数据
    expect(html).toContain("fav-unavailable");
    expect(html).not.toContain("data-row-path");
    expect(listMock).not.toHaveBeenCalled();
  });

  it("详细信息·在线成员:节头落点并入单表,加载中标注,零拉取", () => {
    const fav = favOf("f-det-on", [{ device: "dev-1", path: "C:\\a" }]);
    const html = renderToStaticMarkup(
      createElement(FavView, {
        ...baseProps(fav),
        viewMode: "details",
        devices: [{ ...offlineDev("dev-1"), online: true }],
      }),
    );
    expect((html.match(/<thead/g) ?? []).length).toBe(1);
    expect(html).toContain(`data-drop-loc="dev-1\u0000C:\\a"`); // 在线节落点
    expect((html.match(/data-drop-loc/g) ?? []).length).toBe(2); // 宿主+节头
    expect((html.match(/"caret open"/g) ?? []).length).toBe(1);
    expect(html).toContain("加载中…");
    expect(listMock).not.toHaveBeenCalled();
  });

  it("§16.2 零色点:任一视图模式均无 src-dot 词面(主区色标全量移除)", () => {
    for (const viewMode of ["list", "details", "tiles"] as const) {
      const fav = favOf("f-dot", [
        { device: "dev-1", path: "C:\\a" },
        { device: "dev-2", path: "/b" },
      ]);
      const html = renderToStaticMarkup(
        createElement(FavView, { ...baseProps(fav), viewMode }),
      );
      expect(html).not.toContain("src-dot");
    }
  });

  it("§16.1 组织段词面退场:主区无「混合」切换面(viewOrg 消费已删)", () => {
    const fav = favOf("f-org", [{ device: "dev-1", path: "C:\\a" }]);
    const html = renderToStaticMarkup(createElement(FavView, baseProps(fav)));
    // 分组节头恒在(分类恒定),不存在平铺混合分支
    expect(html).toContain("fav-group-head");
  });
});
