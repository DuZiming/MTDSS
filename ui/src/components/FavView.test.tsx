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
});
