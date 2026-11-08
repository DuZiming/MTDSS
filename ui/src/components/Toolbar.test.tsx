/// Toolbar 测试(V3-3 Task 10;返工波 4 §16.1 重写:「查看」菜单收敛为
/// 三视图单段——组织段(混合|分组)与 favViewOrg 偏好持久化已随分类恒
/// 定整体移除。沿 ApprovalModal.test.tsx SSR 手法 + 返工 §13.2(R5)
/// 「新建」下拉——newMenuSections 纯函数单项「快捷目录」+ menuitem 动作
/// 语义 + 整栏 newEnabled 门控(禁用态无下拉、启用态三触发钮)。菜单展开
/// 态是 Toolbar 内部 state(SSR 首帧恒收起,MenuGroup open=false 不渲染
/// 菜单体),故分段构建抽为纯函数测试直驱,菜单体词面/radio 语义经
/// MenuGroup(open=true)SSR 钉死;整栏 SSR 钉「查看」菜单零组织词面
/// (分类恒定锚:目录/设备/快捷位一律无组织段)。
import { createElement, type ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import "../i18n";

import i18n from "../i18n";
import Toolbar, { MenuGroup, newMenuSections, viewMenuSections, type MenuSection } from "./Toolbar";

const noop = () => {};

/// i18n 词表取词(zh 内联装配,与 main.tsx 同源)。
const t = ((key: string) => i18n.t(key)) as (key: string) => string;

/// 「查看」菜单分段(与 Toolbar 内构建同参)。
const sectionsOf = (): MenuSection[] => viewMenuSections(t, "list", noop, (fn) => fn);

/// 整栏 props(与 App 下发同形;R5 起 newEnabled/onNewFav 必传;V3-4 §5
/// 起 deleteEnabled/onOpenSettings 必传)。
const baseBar = (newEnabled: boolean): ComponentProps<typeof Toolbar> => ({
  sortState: { key: "name", dir: "asc" },
  onSortState: noop,
  viewMode: "list",
  onViewMode: noop,
  opsEnabled: false,
  pasteEnabled: false,
  theme: "dark",
  onCycleTheme: noop,
  onCut: noop,
  onCopy: noop,
  onPaste: noop,
  onDelete: noop,
  newEnabled,
  onNewFav: noop,
  deleteEnabled: true,
  onOpenSettings: noop,
});

/// 展开态菜单 SSR(真实 MenuGroup,钉 menuitemradio/aria-checked 标记)。
const menuHtml = (sections: MenuSection[]): string =>
  renderToStaticMarkup(
    createElement(MenuGroup, { label: "查看", icon: null, sections, open: true, onToggle: noop }),
  );

/// 提取含指定词面的 menuitemradio 片段(按钮无嵌套,非贪婪到首个闭合)。
const radioItem = (html: string, label: string): string => {
  const items = html.match(/<button[^>]*role="menuitemradio"[^>]*>[\s\S]*?<\/button>/g) ?? [];
  const hit = items.find((b) => b.includes(label));
  expect(hit, `无含「${label}」的 menuitemradio`).toBeDefined();
  return hit as string;
};

describe("viewMenuSections(§16.1 收敛:三视图单段)", () => {
  it("仅三视图段(零回归锚):无组织段标头,平铺/列表/详细信息 checked 随 viewMode", () => {
    const secs = sectionsOf();
    expect(secs).toHaveLength(1);
    expect(secs[0].label).toBeUndefined();
    expect(secs[0].items.map((it) => it.label)).toEqual(["平铺", "列表", "详细信息"]);
    expect(secs[0].items.map((it) => it.checked)).toEqual([false, true, false]);
    // 组织段词面全量退场(分类恒定:主区恒分组,无切换面)
    const labels = secs.flatMap((s) => s.items.map((it) => it.label));
    expect(labels).not.toContain("混合");
    expect(labels).not.toContain("分组");
    expect(labels).not.toContain("组织");
  });

  it("展开态:仅三视图 radio 勾选标注,无组织词面(零回归锚)", () => {
    const html = menuHtml(sectionsOf());
    expect(html).toContain("详细信息");
    expect(radioItem(html, "列表")).toContain('aria-checked="true"');
    expect(radioItem(html, "平铺")).toContain('aria-checked="false"');
    expect(html).not.toContain("组织");
    expect(html).not.toContain("混合");
  });
});

describe("Toolbar 整栏 SSR(§16.1 零回归锚)", () => {
  it("整栏照常渲染,「查看」钮在位,无组织段词面", () => {
    const html = renderToStaticMarkup(createElement(Toolbar, baseBar(true)));
    expect(html).toContain("工具栏");
    expect(html).toContain("查看");
    expect(html).toContain("新建");
    expect(html).not.toContain("组织");
    expect(html).not.toContain("混合");
    expect(html).not.toContain("分组");
  });
});

describe("Toolbar 新建下拉(返工 §13.2,R5)", () => {
  it("newEnabled=true:新建为下拉触发钮(排序/查看外第三处 aria-haspopup),占位词面退场", () => {
    const html = renderToStaticMarkup(createElement(Toolbar, baseBar(true)));
    // 触发钮三处:新建 + 排序 + 查看(V2 占位钮无下拉语义)
    expect(html.match(/aria-haspopup="menu"/g)).toHaveLength(3);
    // 占位 tag 与 disabledHint 全量下线(激活后不再是占位)
    expect(html).not.toContain("占位");
    expect(html).not.toContain("V2 提供");
    // 只读禁用提示不在启用态出现
    expect(html).not.toContain("快捷目录为只读,无法新建");
    // 菜单体首帧收起(SSR open=false 不渲染)→ 无菜单体/菜单项词面
    expect(html).not.toContain('role="menu"');
    expect(html).not.toContain("快捷目录");
  });

  it("newEnabled=false(未登录或 fav 只读):新建钮禁用,仅两处下拉触发", () => {
    const html = renderToStaticMarkup(createElement(Toolbar, baseBar(false)));
    expect(html.match(/aria-haspopup="menu"/g)).toHaveLength(2);
    // 禁用说明(title+aria-label 同词,沿 favoritesNewBlocked 语义改写)
    expect(html).toContain("快捷目录为只读,无法新建");
    expect(html).toContain("新建");
    // 无菜单体
    expect(html).not.toContain('role="menu"');
  });
});

describe("newMenuSections(返工 §13.2 新建菜单分段)", () => {
  it("单段单项「快捷目录」(键 fav),onPick 直驱 onNewFav(wrap=恒等)", () => {
    let picked = 0;
    const secs = newMenuSections(
      t,
      () => {
        picked += 1;
      },
      (fn) => fn,
    );
    expect(secs).toHaveLength(1);
    expect(secs[0].label).toBeUndefined();
    expect(secs[0].items).toHaveLength(1);
    expect(secs[0].items[0].key).toBe("fav");
    expect(secs[0].items[0].label).toBe("快捷目录");
    secs[0].items[0].onPick();
    expect(picked).toBe(1);
  });
});

describe("MenuGroup 动作菜单渲染(返工 §13.2 menuitem 语义)", () => {
  it("itemRole=menuitem:role=menuitem,无勾选列/aria-checked,词面快捷目录", () => {
    const html = renderToStaticMarkup(
      createElement(MenuGroup, {
        label: "新建",
        icon: null,
        itemRole: "menuitem",
        sections: newMenuSections(t, noop, (fn) => fn),
        open: true,
        onToggle: noop,
      }),
    );
    expect(html).toContain('role="menuitem"');
    expect(html).not.toContain("menuitemradio");
    expect(html).not.toContain("aria-checked");
    expect(html).not.toContain("tb-check");
    expect(html).toContain("快捷目录");
  });
});

describe("Toolbar 设置齿轮与删除门(V3-4 §5)", () => {
  /// 提取 aria-label 命中的按钮片段(按钮无嵌套,非贪婪到首个闭合)。
  const btnByLabel = (html: string, label: string): string => {
    const items = html.match(/<button[^>]*>[\s\S]*?<\/button>/g) ?? [];
    const hit = items.find((b) => b.includes(`aria-label="${label}"`));
    expect(hit, `无 aria-label=${label} 的按钮`).toBeDefined();
    return hit as string;
  };

  it("设置齿轮钮在位(aria-label=设置,V3-4 §5-0)", () => {
    const html = renderToStaticMarkup(createElement(Toolbar, baseBar(true)));
    expect(html).toContain("设置");
    btnByLabel(html, "设置");
  });

  it("deleteEnabled=false + opsEnabled=true:删除钮禁用且 title 指路设置(入口显隐)", () => {
    const html = renderToStaticMarkup(
      createElement(Toolbar, { ...baseBar(true), opsEnabled: true, deleteEnabled: false }),
    );
    const del = btnByLabel(html, "删除");
    expect(del).toContain("disabled");
    expect(del).toContain("已禁止删除远端文件");
  });

  it("deleteEnabled=true:删除钮可用(门面不误伤本机位)", () => {
    const html = renderToStaticMarkup(
      createElement(Toolbar, { ...baseBar(true), opsEnabled: true, deleteEnabled: true }),
    );
    const del = btnByLabel(html, "删除");
    expect(del).not.toContain("disabled");
    expect(del).not.toContain("已禁止删除远端文件");
  });
});
