/// AddressBar SSR 冒烟(V3-3 返工 R6,spec §13.6 快捷命名空间;返工二
/// §14.3 命名空间修正;返工波 4 §15.5 三件套;返工裁定五 §17.1 favs 着陆
/// 位;返工裁定六 §18.1 chip 统一/根段语义、§18.3 ios 图标;沿
/// Sidebar.test.tsx 手法:react-dom/server 纯字符串渲染,不跑 effects,
/// 渲染期不发任何请求)。钉死词面/结构:
/// ①favdir 位=「快捷目录」根 crumb(可点回着陆位,§18.1)+ 快捷名 chip
/// (devchip 同款)+ rel 段,设备名/设备 chip 不在场(命名空间隐藏来源);
/// ②rel="" 成员根无 rel 段(只剩两级);③dir 位零回归(根段+设备 chip+
/// 路径段原样);④fav 位(§14.3 入命名空间):根段「快捷目录」+ 快捷名
/// chip 带下拉(§18.1 统一),「Synshu 设备」/设备词面不在场;⑤收件箱
/// dir 位(§14.3 入命名空间,§18.1 收件箱=chip 当前值):「快捷目录 ▸
/// 收件箱 chip」,recv_dir 段前缀判定("recvx" 非收件箱);⑥inboxBase 在
/// 场但不命中 → 设备位旧形态零回归;⑦搜索框 §15.5:dir/favdir/fav 三位
/// 启用、设备层禁用;复制路径钮 favdir 启用、fav/设备层禁用;⑧favs 着陆
/// 位(§17.1/§18.1):根段「快捷目录」黑色不可点 + 「目录列表」灰态
/// chip(devchip-empty),设备词面不在场,搜索/复制禁用(下拉收合态——
/// SSR 不展开,内容词面钉在 FavsMenuItems 直渲染例);⑨rootCrumbLoc 纯
/// 函数钉根段落点语义(SSR 面上看不见点击目标);⑩平台图标 §18.3:
/// ios/android → 手机形(SVG 几何钉法,同 icons.test.tsx 存档思路),
/// windows → PC 形零回归。
/// 下拉切换与编辑进入/预填为交互,SSR 不可测,不在本文件覆盖(编辑态预
/// 填/解析往返钉在 lib/address.test.ts)。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import "../i18n";

import type { DeviceInfo } from "../lib/derived";
import type { Loc } from "../lib/nav";
import AddressBar, { FavsMenuItems, isInboxPath, rootCrumbLoc } from "./AddressBar";

const devOf = (id: string, name: string, platform: string, online: boolean): DeviceInfo => ({
  device_id: id,
  name,
  platform,
  online,
});

const noop = () => {};

/// 平台图标 SVG 几何钉法(icons.tsx 存档):手机形独有 path「M7.2 12h1.6」
/// (机身下缘听标),PC 形独有 rect「x=2 y=3」(显示器身)。
const PHONE_GLYPH = 'd="M7.2 12h1.6"';
const PC_GLYPH = '<rect x="2" y="3"';

/// 全量 props(与 App.tsx AddressBar 位下发同形;各例按需覆写 current /
/// favCrumbs)。favdir 位 favCrumbs 形 = App 侧 favdirCrumbs 同构。
const baseProps = (current: Loc, platform = "windows") => ({
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
  devices: [devOf("dev-on", "工位机", platform, true)],
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
  inboxLoc: { kind: "dir", device: "dev-on", path: "C:\\recv" } as Loc,
};

/// favs 着陆位 favCrumbs 形(§17.1/§18.1;与 App 侧 favs 分支同构:无当前
/// 选中项 + inboxLoc 目标)。inboxLoc 以 Loc 形下发(kind dir)。
const favsCrumbs = {
  favorites: favdirCrumbs.favorites,
  currentFavId: "",
  currentFavName: "",
  relSegments: [] as string[],
  inboxLoc: { kind: "dir", device: "dev-on", path: "C:\\recv" } as Loc,
};

/// 收件箱位 favCrumbs 形(§18.1:全量清单 + inboxLoc + inboxBase——App 侧
/// 收件箱分支同构)。
const inboxCrumbs = {
  favorites: favdirCrumbs.favorites,
  currentFavId: "",
  currentFavName: "",
  relSegments: [] as string[],
  inboxBase: "C:\\recv",
  inboxLoc: { kind: "dir", device: "dev-on", path: "C:\\recv" } as Loc,
};

/// 按 aria-label 定位元素并断言禁用态(SSR:disabled 属性在场 = 禁用)。
const fieldDisabled = (html: string, tag: "input" | "button", ariaLabel: string): boolean => {
  const i = html.indexOf(`aria-label="${ariaLabel}"`);
  if (i < 0) return false;
  const start = html.lastIndexOf("<" + tag, i);
  return html.slice(start, i).includes("disabled");
};

describe("AddressBar SSR 冒烟(V3-3 返工 R6:快捷命名空间)", () => {
  it("favdir 位:快捷目录根 crumb(可点回着陆)+ 快捷名 chip + rel 段,设备名/词面隐藏", () => {
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
    // 根 crumb=「快捷目录」(非「Synshu 设备」——无设备层语义);可点回
    // 着陆位(§18.1:落点语义钉在 rootCrumbLoc 例,此处钉可点形态)
    expect(html).toContain("快捷目录");
    expect(html).toContain("crumb-link");
    expect(html).not.toContain(">快捷目录</span>");
    expect(html).not.toContain("Synshu 设备");
    // 快捷名 chip(§18.1 统一 devchip 渲染器,非灰态=有当前实体)
    expect(html).toContain('"devchip"');
    expect(html).not.toContain("devchip-empty");
    expect(html).toContain('devchip-name">项目</span>');
    // rel 段在场;末段=当前位粗体不可点(crumb cur)
    expect(html).toContain("src");
    expect(html).toContain("深层");
    expect(html).toContain('>深层</span>');
    // 设备词面全不在场(§13.6:设备与原始路径隐藏;「设备列表」= 设备层
    // 灰态词面,同样不入命名空间位)
    expect(html).not.toContain("工位机");
    expect(html).not.toContain("设备列表");
    expect(html).not.toContain("目录列表");
    // 原始路径不露(base 不入面)
    expect(html).not.toContain("C:\\docs");
    // 旧 favchip 独立类废止(§18.1 一枚渲染器)
    expect(html).not.toContain("favchip");
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
    expect(html).toContain('"devchip"');
    expect(html).toContain('devchip-name">项目</span>');
    expect(html).not.toContain("工位机");
    expect(html).not.toContain("设备列表");
  });

  it("dir 位零回归:根段+设备 chip+路径段原样,快捷命名空间词面不在场", () => {
    const html = renderToStaticMarkup(
      createElement(AddressBar, baseProps({ kind: "dir", device: "dev-on", path: "C:\\docs\\src" }))
    );
    expect(html).toContain("Synshu 设备");
    expect(html).toContain('"devchip"');
    expect(html).toContain("工位机");
    expect(html).toContain("docs");
    expect(html).toContain("src");
    // 快捷命名空间词面不在场(根段/着陆 chip 双词面)
    expect(html).not.toContain("快捷目录");
    expect(html).not.toContain("目录列表");
    expect(html).not.toContain("收件箱");
  });

  it("fav 位(§14.3/§18.1):根段快捷目录(可点回着陆)+ 快捷名 chip,设备词面不在场", () => {
    const html = renderToStaticMarkup(
      createElement(AddressBar, {
        ...baseProps({ kind: "fav", id: "f-1" }),
        favCrumbs: { ...favdirCrumbs, relSegments: [] },
      })
    );
    // 根段=「快捷目录」可点(§18.1:回着陆位;落点钉在 rootCrumbLoc 例)
    expect(html).toContain("快捷目录");
    expect(html).toContain("crumb-link");
    expect(html).not.toContain(">快捷目录</span>");
    expect(html).not.toContain("Synshu 设备");
    // 快捷名 chip 在场(devchip 统一渲染器,§18.1)
    expect(html).toContain('"devchip"');
    expect(html).not.toContain("devchip-empty");
    expect(html).toContain('devchip-name">项目</span>');
    // 设备词面不在场(§13.6 同口径:来源不入面包屑)
    expect(html).not.toContain("工位机");
    expect(html).not.toContain("设备列表");
    // 第二级在场:根段▸chip 分隔符
    expect(html).toContain('"csep"');
    // rel 段不在场(fav 位无命名空间下级,relSegments 传空)
    expect(html).not.toContain(">src<");
    expect(html).not.toContain(">深层<");
  });

  it("收件箱位(§14.3/§18.1,path===inboxBase):快捷目录根 + 收件箱 chip,无裸段", () => {
    const html = renderToStaticMarkup(
      createElement(AddressBar, {
        ...baseProps({ kind: "dir", device: "dev-on", path: "C:\\recv" }),
        favCrumbs: inboxCrumbs,
      })
    );
    // 根段=「快捷目录」可点回着陆位(§18.1)
    expect(html).toContain("快捷目录");
    expect(html).toContain("crumb-link");
    // 收件箱=chip 当前值(devchip 统一渲染器,非灰态;不再是裸段)
    expect(html).toContain('"devchip"');
    expect(html).not.toContain("devchip-empty");
    expect(html).toContain('devchip-name">收件箱</span>');
    expect(html).toContain('aria-expanded');
    // 收件箱根位无下级段(recv_dir 自身不再作路径段;「收件箱」词面只落
    // chip 的 title+名两处,无裸段/可点 crumb 形)
    expect(html).not.toContain(">收件箱</button>");
    // 设备 chip/词面隐藏(命名空间位同口径)
    expect(html).not.toContain("工位机");
    expect(html).not.toContain("Synshu 设备");
    expect(html).not.toContain("设备列表");
  });

  it("收件箱下级(§18.1):快捷目录 ▸ 收件箱 chip ▸ 下级段,末段粗体不可点", () => {
    const html = renderToStaticMarkup(
      createElement(AddressBar, {
        ...baseProps({ kind: "dir", device: "dev-on", path: "C:\\recv\\子\\深" }),
        favCrumbs: inboxCrumbs,
      })
    );
    expect(html).toContain("快捷目录");
    // 收件箱=chip 当前值(带下拉)
    expect(html).toContain('devchip-name">收件箱</span>');
    // 下级段照常:子(可点)、深(当前位粗体不可点)
    expect(html).toContain(">子</button>");
    expect(html).toContain(">深</span>");
    // recv_dir 自身段不再作首段:「收件箱」词面恰落 chip 的 title+名两处
    // (旧形首段还会多一处 crumb 面)
    expect(html.match(/收件箱/g)?.length).toBe(2);
  });

  it("段前缀判定(§14.3):recvx 非收件箱,inboxBase 在场不命中落设备位", () => {
    const html = renderToStaticMarkup(
      createElement(AddressBar, {
        ...baseProps({ kind: "dir", device: "dev-on", path: "C:\\recvx" }),
        favCrumbs: inboxCrumbs,
      })
    );
    // 手工字符串前缀("C:\recv" 命中 "C:\recvx")是禁手:段前缀不匹配 → 旧形态
    expect(html).toContain("Synshu 设备");
    expect(html).toContain('"devchip"');
    expect(html).not.toContain("收件箱");
    expect(html).not.toContain("快捷目录");
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

  it("favs 着陆位(§17.1/§18.1):根段黑色不可点 + 「目录列表」灰态 chip,设备词面不在场", () => {
    const html = renderToStaticMarkup(
      createElement(AddressBar, { ...baseProps({ kind: "favs" }), favCrumbs: favsCrumbs })
    );
    // 根 crumb=「快捷目录」即当前位:黑色粗体不可点(§18.1,同设备层
    // 「Synshu 设备」形;crumb-link 不在场)
    expect(html).toContain("快捷目录");
    expect(html).toContain(">快捷目录</span>");
    expect(html).not.toContain("crumb-link");
    expect(html).not.toContain("Synshu 设备");
    // 目录 chip=「目录列表」(§18.1 裁定词面;devchip-empty 灰态=镜像设备
    // 层「设备列表」,「当前位无实体」)
    expect(html).toContain('"devchip devchip-empty"');
    expect(html).toContain('devchip-name">目录列表</span>');
    expect(html).not.toContain("选择快捷目录…");
    expect(html).not.toContain("favchip");
    expect(html).not.toContain("工位机");
    // 下拉收合态(SSR 不展开):收件箱/快捷名词面不在面包屑本面
    expect(html).not.toContain("收件箱");
    expect(html).not.toContain(">项目<");
    // 第二级分隔符在场(根段▸chip)
    expect(html).toContain('"csep"');
    // 着陆页无目录语境:搜索/复制路径均禁用
    expect(fieldDisabled(html, "input", "搜索当前目录")).toBe(true);
    expect(fieldDisabled(html, "button", "复制路径")).toBe(true);
  });

  it("零回归面:fav/favdir 位 chip=当前快捷名(非着陆词面),快速对表", () => {
    // fav 位:仍显示快捷名 chip(非「目录列表」)
    const favHtml = renderToStaticMarkup(
      createElement(AddressBar, {
        ...baseProps({ kind: "fav", id: "f-1" }),
        favCrumbs: { ...favdirCrumbs, relSegments: [] },
      })
    );
    expect(favHtml).toContain('devchip-name">项目</span>');
    expect(favHtml).not.toContain("目录列表");
    expect(favHtml).not.toContain("devchip-empty");
    // favdir 位:快捷名 chip + rel 段,原样
    const fdHtml = renderToStaticMarkup(
      createElement(AddressBar, {
        ...baseProps({
          kind: "favdir",
          favId: "f-1",
          device: "dev-on",
          base: "C:\\docs",
          rel: "src",
        }),
        favCrumbs: { ...favdirCrumbs, relSegments: ["src"] },
      })
    );
    expect(fdHtml).toContain('devchip-name">项目</span>');
    expect(fdHtml).not.toContain("目录列表");
    expect(fdHtml).toContain(">src<");
  });

  it("设备命名空间零回归(§18.1 统一渲染器):设备层=根段黑色不可点+「设备列表」灰态 chip", () => {
    const html = renderToStaticMarkup(createElement(AddressBar, baseProps({ kind: "devices" })));
    // 根段=「Synshu 设备」即当前位:黑色粗体不可点(与 favs 着陆位根段同形)
    expect(html).toContain(">Synshu 设备</span>");
    expect(html).not.toContain("crumb-link");
    // 设备 chip=「设备列表」灰态(devchip-empty;与 favs 着陆位「目录列表」
    // 灰态同一枚渲染器互为镜像)
    expect(html).toContain('"devchip devchip-empty"');
    expect(html).toContain('devchip-name">设备列表</span>');
    expect(html).not.toContain("快捷目录");
    expect(html).not.toContain("目录列表");
  });

  it("设备命名空间零回归:设备目录位=根段可点回设备层+设备名 chip 非灰态", () => {
    const html = renderToStaticMarkup(
      createElement(AddressBar, baseProps({ kind: "dir", device: "dev-on", path: "C:\\docs" }))
    );
    // 根段=「Synshu 设备」可点回设备层
    expect(html).toContain(">Synshu 设备</button>");
    // 设备 chip=设备名,非灰态(有当前实体;统一渲染器同 fav 名 chip 形)
    expect(html).toContain('"devchip"');
    expect(html).not.toContain("devchip-empty");
    expect(html).toContain('devchip-name">工位机</span>');
  });
});

describe("rootCrumbLoc 纯函数(§18.1 根段落点语义;SSR 面上看不见点击目标)", () => {
  it("设备层与 favs 着陆位=null(即当前位,黑色不可点)", () => {
    expect(rootCrumbLoc({ kind: "devices" })).toBeNull();
    expect(rootCrumbLoc({ kind: "favs" })).toBeNull();
  });

  it("fav/favdir 位=favs 着陆位(§18.1:favdir 曾回 fav 混合视图、fav 曾回设备层,均废)", () => {
    expect(rootCrumbLoc({ kind: "fav", id: "f-1" })).toEqual({ kind: "favs" });
    expect(
      rootCrumbLoc({ kind: "favdir", favId: "f-1", device: "dev-on", base: "C:\\docs", rel: "src" })
    ).toEqual({ kind: "favs" });
  });

  it("收件箱 dir 位(命中)=favs 着陆位;非命中 dir 位=设备层(零回归)", () => {
    expect(rootCrumbLoc({ kind: "dir", device: "dev-on", path: "C:\\recv" }, "C:\\recv")).toEqual({
      kind: "favs",
    });
    expect(
      rootCrumbLoc({ kind: "dir", device: "dev-on", path: "C:\\recv\\子" }, "C:\\recv")
    ).toEqual({ kind: "favs" });
    // inboxBase 在场但段前缀不命中(recvx 不被 "recv" 吞)→ 设备层
    expect(rootCrumbLoc({ kind: "dir", device: "dev-on", path: "C:\\recvx" }, "C:\\recv")).toEqual({
      kind: "devices",
    });
    // 无 inboxBase → 设备层
    expect(rootCrumbLoc({ kind: "dir", device: "dev-on", path: "C:\\docs" })).toEqual({
      kind: "devices",
    });
    expect(rootCrumbLoc({ kind: "dir", device: "dev-on", path: "C:\\docs" }, "")).toEqual({
      kind: "devices",
    });
  });

  it("isInboxPath:段前缀判定(显式分隔符;等价尾分隔形吸收;空基准否)", () => {
    expect(isInboxPath("C:\\recv", "C:\\recv")).toBe(true);
    expect(isInboxPath("C:\\recv\\子\\深", "C:\\recv\\")).toBe(true); // 尾分隔符等价形
    expect(isInboxPath("C:\\recvx", "C:\\recv")).toBe(false);
    expect(isInboxPath("C:\\recv", "C:\\recv\\子")).toBe(false);
    expect(isInboxPath("C:\\recv", "")).toBe(false);
  });
});

describe("平台图标 §18.3(SSR SVG 几何钉法;三处调用点之 AddressBar devchip)", () => {
  it("ios/android → 手机形(§18.3:ios 曾落 PC 形=bug);windows → PC 形零回归", () => {
    const cur = (platform: string) =>
      renderToStaticMarkup(
        createElement(
          AddressBar,
          baseProps({ kind: "dir", device: "dev-on", path: "C:\\docs" }, platform),
        ),
      );
    expect(cur("ios")).toContain(PHONE_GLYPH);
    expect(cur("ios")).not.toContain(PC_GLYPH);
    expect(cur("android")).toContain(PHONE_GLYPH);
    expect(cur("windows")).toContain(PC_GLYPH);
    expect(cur("windows")).not.toContain(PHONE_GLYPH);
  });
});

describe("FavsMenuItems SSR(§18.1 四位统一:收件箱置顶+全部快捷目录+当前勾选)", () => {
  it("收件箱置顶项在前 + 快捷目录项在后;着陆位无勾选(槽位空)", () => {
    const html = renderToStaticMarkup(
      createElement(FavsMenuItems, {
        favorites: [
          { id: "f-1", name: "项目" },
          { id: "f-2", name: "资料" },
        ],
        inboxAvailable: true,
        currentInbox: false,
        currentFavId: "",
        onPickInbox: noop,
        onPickFav: noop,
      })
    );
    const iInbox = html.indexOf("收件箱");
    expect(iInbox).toBeGreaterThan(-1);
    expect(html.indexOf("项目")).toBeGreaterThan(iInbox); // 收件箱置顶
    expect(html).toContain("资料");
    // 设备下拉同款项结构(devmenu-item)
    expect(html).toContain("devmenu-item");
    // 勾选槽为空(着陆位无当前选中项;槽位保留与设备项对齐,无勾选 svg)
    expect(html).not.toMatch(/devmenu-chk"><svg/);
  });

  it("fav/favdir 位:当前快捷项勾选(恰一枚),收件箱与他不勾", () => {
    const html = renderToStaticMarkup(
      createElement(FavsMenuItems, {
        favorites: [
          { id: "f-1", name: "项目" },
          { id: "f-2", name: "资料" },
        ],
        inboxAvailable: true,
        currentInbox: false,
        currentFavId: "f-1",
        onPickInbox: noop,
        onPickFav: noop,
      })
    );
    const checks = html.match(/devmenu-chk"><svg/g) ?? [];
    expect(checks).toHaveLength(1);
    // 勾选落「项目」项(收件箱项之后、资料项之前)
    const iChk = html.indexOf('devmenu-chk"><svg');
    expect(iChk).toBeGreaterThan(html.indexOf("收件箱"));
    expect(iChk).toBeLessThan(html.indexOf("资料"));
  });

  it("收件箱位:收件箱项勾选(置顶首枚),快捷项全不勾", () => {
    const html = renderToStaticMarkup(
      createElement(FavsMenuItems, {
        favorites: [
          { id: "f-1", name: "项目" },
          { id: "f-2", name: "资料" },
        ],
        inboxAvailable: true,
        currentInbox: true,
        currentFavId: "f-1",
        onPickInbox: noop,
        onPickFav: noop,
      })
    );
    const checks = html.match(/devmenu-chk"><svg/g) ?? [];
    expect(checks).toHaveLength(1);
    // 勾选落收件箱项(置顶,「项目」之前)
    const iChk = html.indexOf('devmenu-chk"><svg');
    expect(iChk).toBeGreaterThan(-1);
    expect(iChk).toBeLessThan(html.indexOf("项目"));
  });

  it("收件路径缺席(inboxLoc=null):不列收件箱项;空清单给引导行", () => {
    const html = renderToStaticMarkup(
      createElement(FavsMenuItems, {
        favorites: [],
        inboxAvailable: false,
        currentInbox: false,
        currentFavId: "",
        onPickInbox: noop,
        onPickFav: noop,
      })
    );
    expect(html).not.toContain("收件箱");
    expect(html).toContain("到设备浏览中添加目录");
    expect(html).toContain("devmenu-empty");
  });
});
