/// 「添加到快捷目录」picker SSR 冒烟测试(V3-3 Task 11,沿 FavNameDialog.
/// test.tsx 同款手法):受控组件,react-dom/server 纯字符串渲染,无需 DOM;
/// i18n 经 ../i18n 同步初始化(zh 词表内联,与 main.tsx 同装配)。钉死:
/// open=false 不渲染遮罩;清单渲染名 + 成员数副文;空清单 → 空态文案 +
/// 「新建快捷目录」钮;标题/取消·新建两键全为 zh 词面(不漏原始键面);
/// busy=true 条目与新建钮禁用(addMembers 在途防连点)。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import "../i18n";

import type { Favorite } from "../lib/favorites";
import FavPickerDialog from "./FavPickerDialog";

const noop = () => {};

function fav(id: string, name: string, memberCount: number): Favorite {
  return {
    id,
    name,
    created_at: 1000,
    members: Array.from({ length: memberCount }, (_, i) => ({ device: `d${i}`, path: `C:\\dir${i}` })),
  };
}

const render = (p: {
  open: boolean;
  favorites?: Favorite[];
  busy?: boolean;
}) =>
  renderToStaticMarkup(
    createElement(FavPickerDialog, {
      open: p.open,
      favorites: p.favorites ?? [],
      busy: p.busy ?? false,
      onPick: noop,
      onNew: noop,
      onClose: noop,
    }),
  );

describe("FavPickerDialog(SSR 冒烟:清单词面与空态)", () => {
  it("关闭态:不渲染遮罩与任何词面", () => {
    expect(render({ open: false })).toBe("");
  });

  it("清单态:标题/条目名/成员数副文/取消·新建两键全词面,无原始键面", () => {
    const html = render({ open: true, favorites: [fav("f1", "合同扫描件", 2), fav("f2", "项目资料", 0)] });
    expect(html).toContain("添加到快捷目录");
    expect(html).toContain("合同扫描件");
    expect(html).toContain("2 个成员");
    expect(html).toContain("项目资料");
    expect(html).toContain("0 个成员");
    expect(html).toContain("取消");
    expect(html).toContain("新建快捷目录");
    expect(html).not.toContain("favpicker.");
    expect(html).not.toContain("favorites.");
  });

  it("空清单:空态文案 + 「新建快捷目录」钮,无条目行", () => {
    const html = render({ open: true, favorites: [] });
    expect(html).toContain("还没有快捷目录");
    expect(html).toContain("新建快捷目录");
    expect(html).not.toContain("个成员");
  });

  it("busy=true:条目与新建钮禁用(addMembers 在途防连点)", () => {
    const html = render({ open: true, favorites: [fav("f1", "合同扫描件", 1)], busy: true });
    expect(html).toContain("disabled");
  });
});
