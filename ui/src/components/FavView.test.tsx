/// FavView SSR 冒烟(V3-3 Task 9,沿 FileList.test.tsx 手法:react-dom/
/// server 纯字符串渲染,不跑 effects;渲染期 useFavListing 只做 seedSnapshot
/// 纯分桶——成员全离线时初始即 offline 桶、不发任何请求,daemonList 断言
/// 零调用,渲染确定)。两例钉死词面:①两成员全离线→「N 个成员离线或不可
/// 用」注记行出现且无行数据;②空常用(0 成员)→空态引导「到设备浏览中添
/// 加目录」。另钉宿主 data-drop-loc=`fav\0id`(T12 拖放落点契约)。
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

const offlineDev = (id: string): DeviceInfo => ({
  device_id: id,
  name: id,
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
  colors: new Map<string, string>(),
  viewMode: "list" as const,
  viewOrg: "mixed" as const,
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

describe("FavView SSR 冒烟(V3-3 Task 9)", () => {
  it("两成员全离线:离线注记行+计数词面出现,无行数据,daemonList 零调用", () => {
    const fav = favOf("f-off", [
      { device: "dev-1", path: "C:\\a" },
      { device: "dev-2", path: "/b" },
    ]);
    const html = renderToStaticMarkup(createElement(FavView, baseProps(fav)));
    expect(html).toContain("fav-unavailable");
    expect(html).toContain("2 个成员离线或不可用");
    expect(html).not.toContain("data-row-path"); // 无成员出行(离线不入混排行)
    expect(listMock).not.toHaveBeenCalled(); // 离线成员从不拉取(Review Focus #3)
  });

  it("空常用(0 成员):空态引导词面,无离线注记", () => {
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

  it("分组模式·两成员全离线:两个节头(节序=成员添加序)各带离线标注,无行,底部注记仍在", () => {
    const fav = favOf("f-g-off", [
      { device: "dev-1", path: "C:\\a" },
      { device: "dev-2", path: "/b" },
    ]);
    const html = renderToStaticMarkup(
      createElement(FavView, { ...baseProps(fav), viewOrg: "grouped" }),
    );
    expect((html.match(/fav-group-head/g) ?? []).length).toBe(2);
    expect((html.match(/fav-group-tag/g) ?? []).length).toBe(2);
    // 节头 chevron(返工 §13.5):每节头一枚且默认全展(caret open);
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
    expect(html).toContain(`data-drop-loc="fav\u0000f-g-off"`);
    // 两模式一致:底部不可用注记仍在(T9 保留裁定)
    expect(html).toContain("fav-unavailable");
    expect(html).toContain("2 个成员离线或不可用");
    expect(html).not.toContain("data-row-path"); // 离线节无行
    expect(listMock).not.toHaveBeenCalled();
  });

  it("分组模式·在线成员:节头挂节级拖放落点 device\\u0000path+加载中标注,零拉取", () => {
    const fav = favOf("f-g-on", [{ device: "dev-1", path: "C:\\a" }]);
    const html = renderToStaticMarkup(
      createElement(FavView, {
        ...baseProps(fav),
        viewOrg: "grouped",
        devices: [{ ...offlineDev("dev-1"), online: true }],
      }),
    );
    expect(html).toContain(`data-drop-loc="dev-1\u0000C:\\a"`);
    expect((html.match(/"caret open"/g) ?? []).length).toBe(1); // 节头 chevron(§13.5)
    expect(html).toContain("加载中…");
    expect(html).not.toContain("fav-unavailable"); // pending 不入不可用注记
    expect(listMock).not.toHaveBeenCalled(); // SSR 不跑 effect,零拉取
  });
});
