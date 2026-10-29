/// 图标组件库冒烟测试(SSR):每个导出经 renderToStaticMarkup 渲染
/// 必须产出含 <svg 的字符串(react-dom/server 纯字符串,无需 DOM)。
/// 同时钉住三件事:导出面恰为合同清单、着色族只落 --icon-* 变量、
/// 全家零字面 #hex(纪律:themes.css 之外禁止硬编码颜色)。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import * as Icons from "../components/icons";
import type { IconTintKey } from "../components/icons";

/// 全部导出的公共可渲染面:size 人人可选,tintKey 仅 IcFileTyped 消费
/// (其余图标忽略之,这里统一喂 "pdf" 以便 IcFileTyped 走正常路径)。
type AnyIcon = React.ComponentType<{ size?: number; tintKey?: IconTintKey }>;

/// 合同导出面(task-3-brief 按族全列):后续任务按名消费,清单缺失或
/// 多余均视为破坏合同。
const EXPORT_NAMES = [
  // 导航
  "IcBack", "IcForward", "IcUp", "IcChevronRight", "IcChevronDown",
  // 操作
  "IcCut", "IcCopy", "IcPaste", "IcDelete",
  // 菜单
  "IcSort", "IcView",
  // 主题
  "IcTheme",
  // 地址
  "IcCopyPath", "IcSearch",
  // 平台
  "IcPc", "IcPhone",
  // 抽屉操作
  "IcPause", "IcX", "IcRedo", "IcTrash",
  // 杂项
  "IcCheck",
  // 着色
  "IcFolder", "IcFile", "IcFileTyped",
  // 方向
  "IcDirRecv", "IcDirSend", "IcDirSwap",
];

const icons = Icons as unknown as Record<string, AnyIcon>;

const render = (name: string, props: { size?: number; tintKey?: IconTintKey } = {}) =>
  renderToStaticMarkup(createElement(icons[name], { tintKey: "pdf", ...props }));

describe("图标组件库", () => {
  it("导出面恰为合同清单,且每个导出均可 SSR 渲染并含 <svg", () => {
    expect(Object.keys(icons).sort()).toEqual([...EXPORT_NAMES].sort());
    for (const name of EXPORT_NAMES) {
      expect(typeof icons[name], name).toBe("function");
      expect(render(name).includes("<svg"), name).toBe(true);
    }
  });

  it("零字面色:任何导出输出不含 #hex(fill/stroke 只引变量或 currentColor)", () => {
    for (const name of EXPORT_NAMES) {
      expect(/#[0-9a-f]{3,8}\b/i.test(render(name)), name).toBe(false);
    }
  });

  it("线性族笔画逐值存档:stroke-width 1.4 + stroke currentColor", () => {
    const html = render("IcBack");
    expect(html).toContain('stroke-width="1.4"');
    expect(html).toContain('stroke="currentColor"');
  });

  it("size 透传为 width/height(默认 15;IcChevronDown 存档为 12)", () => {
    expect(render("IcBack")).toContain('width="15"');
    expect(render("IcBack", { size: 20 })).toContain('width="20"');
    expect(render("IcChevronDown")).toContain('width="12"');
  });

  it("IcFolder 落 folder fill/edge 变量,IcFile 落 file body/edge 变量", () => {
    const folder = render("IcFolder");
    expect(folder).toContain("var(--icon-folder-fill)");
    expect(folder).toContain("var(--icon-folder-edge)");
    // 存档 folder()/filetint() 封装不带 S:着色族无描边属性(1px 直角),
    // 与线性族 1.4 圆角刻意不同,勿被根属性统一。
    expect(folder).not.toContain("stroke-width");
    const file = render("IcFile");
    expect(file).toContain("var(--icon-file-body)");
    expect(file).toContain("var(--icon-file-edge)");
    expect(file).not.toContain("stroke-width");
  });

  it("IcFileTyped 按 tintKey 落对应 --icon-tint-* 变量(generic→页缘色)", () => {
    const cases: readonly [IconTintKey, string][] = [
      ["pdf", "var(--icon-tint-pdf)"],
      ["xls", "var(--icon-tint-xls)"],
      ["vid", "var(--icon-tint-vid)"],
      ["exe", "var(--icon-tint-exe)"],
      ["txt", "var(--icon-tint-txt)"],
      ["img", "var(--icon-tint-img)"],
      ["archive", "var(--icon-tint-archive)"],
      // generic(Word/音频/未知/无扩展名):无专属 tint 变量,取页缘色。
      ["generic", "var(--icon-file-edge)"],
    ];
    for (const [tintKey, fill] of cases) {
      const html = renderToStaticMarkup(createElement(Icons.IcFileTyped, { tintKey }));
      expect(html).toContain(fill);
    }
  });
});
