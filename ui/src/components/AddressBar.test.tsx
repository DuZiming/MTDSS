/// AddressBar SSR 冒烟(V3-3 返工 R6,spec §13.6 快捷命名空间;返工二
/// §14.3 命名空间修正;返工波 4 §15.5 三件套;沿 Sidebar.test.tsx 手法:react-dom/server 纯字符
/// 串渲染,不跑 effects,渲染期不发任何请求)。钉死词面/结构:
/// ①favdir 位=「快捷目录」根 crumb + 快捷名 chip + rel 段,设备名/设备
/// chip 不在场(命名空间隐藏来源);②rel="" 成员根无 rel 段(只剩两级);
/// ③dir 位零回归(根段+设备 chip+路径段原样);④fav 位(§14.3 入命名
/// 空间):根段「快捷目录」+ 快捷名 chip 带下拉(补齐 R8「无名段」缺口),
/// 「Synshu 设备」/设备 chip 不在场;⑤收件箱 dir 位(§14.3):「快捷
/// 目录 ▸ 收件箱 ▸ 下级段」,设备 chip 隐藏;recv_dir 段前缀判定
/// ("recvx" 非收件箱);⑥inboxBase 在场但不命中 → 设备位旧形态零回归;
/// ⑦搜索框 §15.5:dir/favdir/fav 三位启用、设备层禁用;复制路径钮 favdir
/// 启用、fav/设备层禁用。下拉切换与编辑进入/预填为交互,SSR 不可测,不在
/// 本文件覆盖(编辑态预填/解析往返钉在 lib/address.test.ts)。
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
    { id: "f-1", name: "项目", members: [{ device: "dev-on", path: "C:\\docs" }] },
    { id: "f-2", name: "资料", members: [{ device: "dev-on", path: "C:\\stuff" }] },
  ],
  currentFavId: "f-1",
  currentFavName: "项目",
  relSegments: ["src", "深层"],
};

/// 按 aria-label 定位元素并断言禁用态(SSR:disabled 属性在场 = 禁用)。
const fieldDisabled = (html: string, tag: "input" | "button", ariaLabel: string): boolean => {
  const i = html.indexOf(`aria-label="${ariaLabel}"`);
  if (i < 0) return false;
  const start = html.lastIndexOf("<" + tag, i);
  return html.slice(start, i).includes("disabled");
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

  it("fav 位(§14.3 入命名空间):根段快捷目录 + 快捷名 chip,设备词面不在场", () => {
    const html = renderToStaticMarkup(
      createElement(AddressBar, {
        ...baseProps({ kind: "fav", id: "f-1" }),
        favCrumbs: { ...favdirCrumbs, relSegments: [] },
      })
    );
    // 根段=「快捷目录」(非「Synshu 设备」;§14.3 命名空间修正,补齐无名段)
    expect(html).toContain("快捷目录");
    expect(html).not.toContain("Synshu 设备");
    // 快捷名 chip 在场(favchip;§14.3:名段带下拉切换)
    expect(html).toContain("favchip");
    expect(html).toContain("项目");
    // 设备 chip 与其「设备列表」词面不在场(§13.6 同口径:来源不入面包屑)
    expect(html).not.toContain('"devchip"');
    expect(html).not.toContain("设备列表");
    // 第二级在场:根段▸chip 分隔符随 favchip 回归(§14.3 前 fav 位撤 csep)
    expect(html).toContain('"csep"');
    // rel 段不在场(fav 位无命名空间下级,relSegments 传空)
    expect(html).not.toContain(">src<");
    expect(html).not.toContain(">深层<");
  });

  it("收件箱位(§14.3,path===inboxBase):快捷目录 ▸ 收件箱,设备 chip 隐藏", () => {
    const html = renderToStaticMarkup(
      createElement(AddressBar, {
        ...baseProps({ kind: "dir", device: "dev-on", path: "C:\\recv" }),
        favCrumbs: { ...favdirCrumbs, relSegments: [], inboxBase: "C:\\recv" },
      })
    );
    // 根段=「快捷目录」+ 首段=「收件箱」(recv_dir 自身段)
    expect(html).toContain("快捷目录");
    expect(html).toContain("收件箱");
    // 收件箱即当前位:粗体不可点(crumb cur);无更下级段
    expect(html).toContain(">收件箱</span>");
    // 设备 chip 隐藏(命名空间位同口径;设备名与「Synshu 设备」均不在场)
    expect(html).not.toContain('"devchip"');
    expect(html).not.toContain("工位机");
    expect(html).not.toContain("Synshu 设备");
  });

  it("收件箱下级(§14.3):快捷目录 ▸ 收件箱 ▸ 下级段,末段粗体不可点", () => {
    const html = renderToStaticMarkup(
      createElement(AddressBar, {
        ...baseProps({ kind: "dir", device: "dev-on", path: "C:\\recv\\子\\深" }),
        favCrumbs: { ...favdirCrumbs, relSegments: [], inboxBase: "C:\\recv" },
      })
    );
    expect(html).toContain("快捷目录");
    // 收件箱段可点(回 recv_dir)
    expect(html).toContain(">收件箱</button>");
    // 下级段照常:子(可点)、深(当前位粗体不可点)
    expect(html).toContain(">子</button>");
    expect(html).toContain(">深</span>");
  });

  it("段前缀判定(§14.3):recvx 非收件箱,inboxBase 在场不命中落设备位", () => {
    const html = renderToStaticMarkup(
      createElement(AddressBar, {
        ...baseProps({ kind: "dir", device: "dev-on", path: "C:\\recvx" }),
        favCrumbs: { ...favdirCrumbs, relSegments: [], inboxBase: "C:\\recv" },
      })
    );
    // 手工字符串前缀("C:\recv" 命中 "C:\recvx")是禁手:段前缀不匹配 → 旧形态
    expect(html).toContain("Synshu 设备");
    expect(html).toContain('"devchip"');
    expect(html).not.toContain("收件箱");
  });

  it("§15.5 搜索框:fav 位启用(input 无 disabled)", () => {
    const html = renderToStaticMarkup(
      createElement(AddressBar, {
        ...baseProps({ kind: "fav", id: "f-1" }),
        favCrumbs: { ...favdirCrumbs, relSegments: [] },
      })
    );
    expect(fieldDisabled(html, "input", "搜索当前目录")).toBe(false);
    // 同位复制路径钮仍禁用(fav 位无单一路径,§15.5)
    expect(fieldDisabled(html, "button", "复制路径")).toBe(true);
  });

  it("§15.5 搜索框/复制路径:favdir 位两者皆启用", () => {
    const current: Loc = {
      kind: "favdir",
      favId: "f-1",
      device: "dev-on",
      base: "C:\\docs",
      rel: "src",
    };
    const html = renderToStaticMarkup(
      createElement(AddressBar, { ...baseProps(current), favCrumbs: favdirCrumbs })
    );
    expect(fieldDisabled(html, "input", "搜索当前目录")).toBe(false);
    expect(fieldDisabled(html, "button", "复制路径")).toBe(false);
  });

  it("§15.5 零回归:dir 位搜索/复制启用,设备层两者禁用", () => {
    const dirHtml = renderToStaticMarkup(
      createElement(AddressBar, baseProps({ kind: "dir", device: "dev-on", path: "C:\\docs" }))
    );
    expect(fieldDisabled(dirHtml, "input", "搜索当前目录")).toBe(false);
    expect(fieldDisabled(dirHtml, "button", "复制路径")).toBe(false);
    const devHtml = renderToStaticMarkup(
      createElement(AddressBar, baseProps({ kind: "devices" }))
    );
    expect(fieldDisabled(devHtml, "input", "搜索当前目录")).toBe(true);
    expect(fieldDisabled(devHtml, "button", "复制路径")).toBe(true);
  });
});
