/// 「存入哪个成员」选择层 SSR 冒烟测试(V3-3 Task 12,沿 FavPickerDialog.
/// test.tsx 同款手法):受控组件,react-dom/server 纯字符串渲染,无需 DOM;
/// i18n 经 ../i18n 同步初始化(zh 词表内联,与 main.tsx 同装配)。钉死:
/// open=false 不渲染遮罩;成员行 = 标志色点 + 设备名 + 路径;离线成员行
/// 禁用(brief Step 1 裁定简化:路由到远端由引擎拒,UI 不预判——离线禁选,
/// 断言 disabled);在线成员可选;标题/取消全为 zh 词面(不漏原始键面)。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import "../i18n";

import type { DeviceInfo } from "../lib/derived";
import type { Favorite } from "../lib/favorites";
import FavMemberPicker from "./FavMemberPicker";

const noop = () => {};

function dev(id: string, online: boolean): DeviceInfo {
  return { device_id: id, name: `${id}-名`, platform: "windows", online };
}

function fav(members: { device: string; path: string }[]): Favorite {
  return { id: "f1", name: "合同扫描件", created_at: 1000, members };
}

const render = (p: {
  open: boolean;
  fav: Favorite;
  devices?: DeviceInfo[];
  colors?: Map<string, string>;
}) =>
  renderToStaticMarkup(
    createElement(FavMemberPicker, {
      open: p.open,
      fav: p.fav,
      devices: p.devices ?? [dev("d1", true), dev("d2", false)],
      colors: p.colors ?? new Map<string, string>([["d1", "#14B8A6"], ["d2", "#06B6D4"]]),
      onPick: noop,
      onClose: noop,
    }),
  );

/// 该设备名所属按钮的开标签(名在按钮内容里,向前找最近的 <button)。
function buttonOpenTag(html: string, nameIdx: number): string {
  return html.slice(html.lastIndexOf("<button", nameIdx), nameIdx);
}

describe("FavMemberPicker(SSR 冒烟:成员行与离线禁选)", () => {
  it("关闭态:不渲染遮罩与任何词面", () => {
    expect(render({ open: false, fav: fav([{ device: "d1", path: "C:\\a" }]) })).toBe("");
  });

  it("成员行 = 色点 + 设备名 + 路径;标题/取消全词面,无原始键面", () => {
    const html = render({ open: true, fav: fav([{ device: "d1", path: "C:\\相册" }]) });
    expect(html).toContain("选择要存入的成员目录");
    expect(html).toContain("d1-名");
    expect(html).toContain("C:\\相册");
    expect(html).toContain("#14B8A6"); // 色点内联背景
    expect(html).toContain("取消");
    expect(html).not.toContain("favorites.");
  });

  it("离线成员行禁用(disabled)且带不可选提示;在线成员可选", () => {
    const html = render({
      open: true,
      fav: fav([
        { device: "d1", path: "C:\\a" },
        { device: "d2", path: "/b" },
      ]),
    });
    expect(html).toContain("disabled");
    expect(html).toContain("设备离线,不可选"); // memberOfflineRow 提示(title)
    expect(html).toContain("d2-名"); // 离线行仍渲染(灰显非隐藏)
    expect(html).toContain("/b");
    const d2Tag = buttonOpenTag(html, html.indexOf("d2-名"));
    expect(d2Tag).toContain("disabled");
    expect(d2Tag).toContain("设备离线,不可选"); // title 落在离线行按钮上
    const d1Tag = buttonOpenTag(html, html.indexOf("d1-名"));
    expect(d1Tag).not.toContain("disabled");
  });

  it("空成员常用(0 成员):清单空,不出成员行", () => {
    const html = render({ open: true, fav: fav([]) });
    expect(html).toContain("选择要存入的成员目录");
    expect(html).not.toContain("fav-member-path"); // 无成员路径行
  });
});
