/// FavsLandingView SSR 冒烟(V3-3 返工裁定五 §17.1 着陆页;沿
/// Sidebar.test.tsx 手法:react-dom/server 纯字符串渲染,不跑 effects)。
/// 钉死结构:①收件箱置顶首行(§17.2 成员语义对齐)在前、快捷目录行在
/// 后,行=文件夹图标+名+成员数副文;②交互面(enterable + tabindex 键盘
/// 回车面)在场;③空清单引导行「左上角「新建」创建快捷目录」;④收件
/// 路径缺席不列收件箱行。双击/回车进分组视图为交互,SSR 不可测,不在
/// 本文件覆盖。
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

describe("FavsLandingView SSR 冒烟(返工裁定五 §17.1:着陆页)", () => {
  it("收件箱置顶首行 + 快捷目录行(名+成员数副文),可进行在场", () => {
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
    // 成员数副文(picker 词面同源;0 成员照示)
    expect(html).toContain("2 个成员");
    expect(html).toContain("0 个成员");
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
