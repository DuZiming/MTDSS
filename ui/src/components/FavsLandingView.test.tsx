/// FavsLandingView SSR 冒烟(V3-3 返工裁定五 §17.1 着陆页;行形返工裁定
/// 六 §18.2 单行同设备行;沿 Sidebar.test.tsx 手法:react-dom/server 纯
/// 字符串渲染,不跑 effects)。钉死结构:①收件箱置顶首行(§17.2 成员语
/// 义对齐)在前、快捷目录行在后,行=文件夹图标+名单行(成员数副文
/// §18.2 撤除,行与 DeviceTable 行同高同形);②交互面(enterable +
/// tabindex 键盘回车面)在场;③空清单引导行「左上角「新建」创建快捷目
/// 录」;④收件路径缺席不列收件箱行。双击/回车进分组视图为交互,SSR 不
/// 可测,不在本文件覆盖。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import "../i18n";

import type { Favorite } from "../lib/favorites";
import FavsLandingView from "./FavsLandingView";

const favOf = (id: string, name: string, memberCount: number): Favorite => ({
  id,
  name,
  created_at: 1,
  members: Array.from({ length: memberCount }, (_, i) => ({
    device: "dev-on",
    path: `C:\\dir${i}`,
  })),
});

const noop = () => {};

describe("FavsLandingView SSR 冒烟(返工裁定五 §17.1;行形返工裁定六 §18.2)", () => {
  it("收件箱置顶首行 + 快捷目录行(单行名,无副文),可进行在场", () => {
    const html = renderToStaticMarkup(
      createElement(FavsLandingView, {
        favorites: [favOf("f-1", "项目", 2), favOf("f-2", "资料", 0)],
        inboxAvailable: true,
        onOpenInbox: noop,
        onOpenFav: noop,
      }),
    );
    // 次序:收件箱(置顶首行)在快捷目录行之前
    const iInbox = html.indexOf("收件箱");
    const iFav = html.indexOf("项目");
    expect(iInbox).toBeGreaterThan(-1);
    expect(iFav).toBeGreaterThan(iInbox);
    expect(html).toContain("资料");
    // 单行名(§18.2 同形):成员数副文撤除——0 成员行也不示数;副文类
    // (cell-sub)与两行文本面(cell-text/cell-title)一并不在场
    expect(html).not.toContain("个成员");
    expect(html).not.toContain("cell-sub");
    expect(html).not.toContain("cell-text");
    expect(html).toContain(">项目</span>");
    expect(html).toContain(">资料</span>");
    // 可进行(设备表同款 enterable)+ 键盘回车面(tabindex)
    expect(html).toContain("enterable");
    expect(html).toContain('tabindex="0"');
  });

  it("空清单:引导行「左上角「新建」创建快捷目录」", () => {
    const html = renderToStaticMarkup(
      createElement(FavsLandingView, {
        favorites: [],
        inboxAvailable: false,
        onOpenInbox: noop,
        onOpenFav: noop,
      }),
    );
    expect(html).toContain("左上角「新建」创建快捷目录");
    // 收件路径缺席:收件箱行不渲染
    expect(html).not.toContain("收件箱");
    expect(html).toContain("empty");
  });

  it("收件路径在场 + 空清单:收件箱行独占,引导行仍在", () => {
    const html = renderToStaticMarkup(
      createElement(FavsLandingView, {
        favorites: [],
        inboxAvailable: true,
        onOpenInbox: noop,
        onOpenFav: noop,
      }),
    );
    expect(html).toContain("收件箱");
    expect(html).toContain("左上角「新建」创建快捷目录");
  });
});
