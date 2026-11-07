/// 侧栏快捷行右键菜单 SSR 冒烟测试(V3-3 Task 13,沿 FavNameDialog.
/// test.tsx 同款手法):react-dom/server 纯字符串渲染,无需 DOM。组件不持
/// i18n(词面由 App 装配进 items)。钉死:items 逐项渲染 label 词面;
/// danger 项挂 danger 类(红字),普通项不带;items 空 → 不渲染(防御,
/// 他端删除快捷后 App 装配出空清单的兜底面)。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import TreeNodeMenu from "./TreeNodeMenu";

const noop = () => {};

const render = (items: { key: string; label: string; danger?: boolean; onPick(): void }[]) =>
  renderToStaticMarkup(createElement(TreeNodeMenu, { x: 10, y: 20, items, onClose: noop }));

describe("TreeNodeMenu(SSR 冒烟:label 渲染/danger 类/空清单防御)", () => {
  it("items 逐项渲染 label 词面", () => {
    const html = render([
      { key: "open", label: "打开", onPick: noop },
      { key: "rename", label: "重命名", onPick: noop },
    ]);
    expect(html).toContain("打开");
    expect(html).toContain("重命名");
    expect(html).toContain('role="menu"');
  });

  it("danger 项挂 danger 类(红字),普通项不带", () => {
    const html = render([
      { key: "open", label: "打开", onPick: noop },
      { key: "delete", label: "删除快捷", danger: true, onPick: noop },
    ]);
    const delIdx = html.indexOf("删除快捷");
    const openIdx = html.indexOf("打开");
    const delTag = html.slice(html.lastIndexOf("<button", delIdx), delIdx);
    const openTag = html.slice(html.lastIndexOf("<button", openIdx), openIdx);
    expect(delTag).toContain("danger");
    expect(openTag).not.toContain("danger");
  });

  it("items 空 → 不渲染(防御)", () => {
    expect(render([])).toBe("");
  });
});
