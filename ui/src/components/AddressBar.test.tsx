/// AddressBar SSR 冒烟(V3-3 返工 R6,spec §13.6 快捷命名空间;沿
/// Sidebar.test.tsx 手法:react-dom/server 纯字符串渲染,不跑 effects,
/// 渲染期不发任何请求)。钉死词面/结构:①favdir 位=「快捷目录」根 crumb
/// + 快捷名 chip + rel 段,设备名/设备 chip 不在场(命名空间隐藏来源);
/// ②rel="" 成员根无 rel 段(只剩两级);③dir 位零回归(根段+设备 chip+
/// 路径段原样);④fav 位零回归(「设备列表」chip 原样,字节不变)。
/// 下拉切换/编辑禁入为交互,SSR 不可测,不在本文件覆盖(纯结构断言)。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import "../i18n";

import type { DeviceInfo } from "../lib/derived";
import type { Loc } from "../lib/nav";
import AddressBar from "./AddressBar";

const devOf = (id: string, name: string, online: boolean): DeviceInfo => ({
  device_id: id,
  name,
  platform: "windows",
  online,
});

const noop = () => {};

/// 全量 props(与 App.tsx AddressBar 位下发同形;各例按需覆写 current /
/// favCrumbs)。favdir 位 favCrumbs 形 = App 侧 favdirCrumbs 同构。
const baseProps = (current: Loc) => ({
  current,
  canBack: true,
  canForward: false,
  canUp: true,
  onBack: noop,
  onForward: noop,
  onUp: noop,
  onNavigate: noop,
  deviceName: (id: string) => (id === "dev-on" ? "工位机" : id),
  searchQ: "",
  onSearchChange: noop,
  dropHoverKey: null,
  devices: [devOf("dev-on", "工位机", true)],
  onAddressSubmit: noop,
  onCopyPath: noop,
  modalBlocked: false,
});

const favdirCrumbs = {
  favorites: [
    { id: "f-1", name: "项目" },
    { id: "f-2", name: "资料" },
  ],
  currentFavId: "f-1",
  currentFavName: "项目",
  relSegments: ["src", "深层"],
};

describe("AddressBar SSR 冒烟(V3-3 返工 R6:快捷命名空间)", () => {
  it("favdir 位:快捷目录根 crumb + 快捷名 chip + rel 段,设备名/chip 隐藏", () => {
    const current: Loc = {
      kind: "favdir",
      favId: "f-1",
      device: "dev-on",
      base: "C:\\docs",
      rel: "src\\深层",
    };
    const html = renderToStaticMarkup(
      createElement(AddressBar, { ...baseProps(current), favCrumbs: favdirCrumbs })
    );
    // 根 crumb=「快捷目录」(非「Synshu 设备」——无设备层语义)
    expect(html).toContain("快捷目录");
    expect(html).not.toContain("Synshu 设备");
    // 快捷名 chip(favchip,设备 chip 的命名空间替身)
    expect(html).toContain("favchip");
    expect(html).toContain("项目");
    // rel 段在场;末段=当前位粗体不可点(crumb cur)
    expect(html).toContain("src");
    expect(html).toContain("深层");
    expect(html).toContain('>深层</span>');
    // 设备名与设备 chip 不在场(§13.6:设备与原始路径隐藏)
    expect(html).not.toContain("工位机");
    expect(html).not.toContain('"devchip"');
    // 原始路径不露(base 不入面)
    expect(html).not.toContain("C:\\docs");
  });

  it("favdir 成员根(rel=\"\"):两级面包屑,无 rel 段", () => {
    const current: Loc = {
      kind: "favdir",
      favId: "f-1",
      device: "dev-on",
      base: "C:\\docs",
      rel: "",
    };
    const html = renderToStaticMarkup(
      createElement(AddressBar, { ...baseProps(current), favCrumbs: favdirCrumbs })
    );
    expect(html).toContain("快捷目录");
    expect(html).toContain("favchip");
    expect(html).toContain("项目");
    expect(html).not.toContain("工位机");
    expect(html).not.toContain('"devchip"');
  });

  it("dir 位零回归:根段+设备 chip+路径段原样", () => {
    const html = renderToStaticMarkup(
      createElement(AddressBar, baseProps({ kind: "dir", device: "dev-on", path: "C:\\docs\\src" }))
    );
    expect(html).toContain("Synshu 设备");
    expect(html).toContain('"devchip"');
    expect(html).toContain("工位机");
    expect(html).toContain("docs");
    expect(html).toContain("src");
    expect(html).not.toContain("favchip");
  });

  it("fav 位零回归:「设备列表」chip 原样(字节不变)", () => {
    const html = renderToStaticMarkup(
      createElement(AddressBar, baseProps({ kind: "fav", id: "f-1" }))
    );
    expect(html).toContain("Synshu 设备");
    expect(html).toContain("设备列表");
    expect(html).toContain('"devchip"');
    expect(html).not.toContain("favchip");
  });
});
