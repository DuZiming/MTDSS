/// Toolbar 测试(V3-3 Task 10,沿 ApprovalModal.test.tsx SSR 手法 +
/// theme.test.ts 的 localStorage 桩):「查看」菜单的 混合|分组 组织段与
/// favViewOrg 全局偏好持久化。菜单展开态是 Toolbar 内部 state(SSR 首帧
/// 恒收起,MenuGroup open=false 不渲染菜单体),故分段构建抽为纯函数
/// viewMenuSections 测试直驱,菜单体词面/radio 语义经 MenuGroup(open=
/// true)SSR 钉死;整栏 SSR 钉 orgToggleVisible=false 时无组织段词面
/// (目录/设备位零回归锚)。localStorage 桩:node 环境无 localStorage,
/// 挂同构实现让持久化用例确定性成立(不为此引 jsdom)。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it } from "vitest";

import "../i18n";

import i18n from "../i18n";
import Toolbar, {
  MenuGroup,
  loadFavViewOrg,
  saveFavViewOrg,
  viewMenuSections,
  type MenuSection,
  type ViewOrg,
} from "./Toolbar";

const noop = () => {};

/// i18n 词表取词(zh 内联装配,与 main.tsx 同源)。
const t = ((key: string) => i18n.t(key)) as (key: string) => string;

/// 「查看」菜单分段(与 Toolbar 内构建同参)。
const sectionsOf = (visible: boolean, viewOrg: ViewOrg): MenuSection[] =>
  viewMenuSections(t, "list", viewOrg, visible, noop, noop, (fn) => fn);

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

describe("viewMenuSections(V3-3 Task 10 混合|分组段)", () => {
  it("orgToggleVisible=false:仅三视图段(零回归锚,无组织段)", () => {
    const secs = sectionsOf(false, "mixed");
    expect(secs).toHaveLength(1);
    expect(secs[0].label).toBeUndefined();
    expect(secs[0].items.map((it) => it.label)).toEqual(["平铺", "列表", "详细信息"]);
    expect(secs[0].items.map((it) => it.checked)).toEqual([false, true, false]);
  });

  it("orgToggleVisible=true:追加「组织」段,混合/分组两项,checked 随 viewOrg", () => {
    for (const org of ["mixed", "grouped"] as const) {
      const secs = sectionsOf(true, org);
      expect(secs).toHaveLength(2);
      expect(secs[1].label).toBe("组织");
      expect(secs[1].items.map((it) => it.label)).toEqual(["混合", "分组"]);
      expect(secs[1].items.map((it) => it.checked)).toEqual(
        org === "mixed" ? [true, false] : [false, true],
      );
    }
  });
});

describe("MenuGroup 组织段渲染(V3-3 Task 10 词面与 radio 语义)", () => {
  it("展开态:组织段标头+两项 menuitemradio,混合勾选/分组未勾", () => {
    const html = menuHtml(sectionsOf(true, "mixed"));
    expect(html).toContain("组织");
    expect(radioItem(html, "混合")).toContain('aria-checked="true"');
    expect(radioItem(html, "分组")).toContain('aria-checked="false"');
  });

  it("展开态·orgToggleVisible=false 的分段渲染无组织词面(零回归锚)", () => {
    const html = menuHtml(sectionsOf(false, "mixed"));
    expect(html).toContain("详细信息");
    expect(html).not.toContain("组织");
    expect(html).not.toContain("混合");
    expect(html).not.toContain("分组");
  });
});

describe("Toolbar 整栏 SSR(V3-3 Task 10)", () => {
  it("orgToggleVisible=false(目录/设备位):整栏照常渲染,无组织段词面", () => {
    const html = renderToStaticMarkup(
      createElement(Toolbar, {
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
        orgToggleVisible: false,
        viewOrg: "mixed",
        onViewOrg: noop,
      }),
    );
    expect(html).toContain("工具栏");
    expect(html).toContain("查看");
    expect(html).not.toContain("组织");
    expect(html).not.toContain("混合");
    expect(html).not.toContain("分组");
  });
});

/// 内存版 localStorage 桩(theme.test.ts 同款:node 环境无 localStorage,
/// 挂同构实现让持久化测试确定性成立)。
const store = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", {
  value: {
    get length(): number {
      return store.size;
    },
    clear(): void {
      store.clear();
    },
    getItem(key: string): string | null {
      return store.has(key) ? (store.get(key) as string) : null;
    },
    key(index: number): string | null {
      return [...store.keys()][index] ?? null;
    },
    removeItem(key: string): void {
      store.delete(key);
    },
    setItem(key: string, value: string): void {
      store.set(key, String(value));
    },
  },
  configurable: true,
  writable: true,
});

describe("loadFavViewOrg / saveFavViewOrg(V3-3 Task 10 全局偏好)", () => {
  it("无记录 → 缺省 mixed", () => {
    expect(loadFavViewOrg()).toBe("mixed");
  });

  it("坏值(「neon」/空串)→ 缺省 mixed", () => {
    localStorage.setItem("favViewOrg", "neon");
    expect(loadFavViewOrg()).toBe("mixed");
    localStorage.setItem("favViewOrg", "");
    expect(loadFavViewOrg()).toBe("mixed");
  });

  it("save→load 回读同值(mixed/grouped 双向)", () => {
    saveFavViewOrg("grouped");
    expect(loadFavViewOrg()).toBe("grouped");
    saveFavViewOrg("mixed");
    expect(loadFavViewOrg()).toBe("mixed");
  });

  it("存储不可用(访问即抛)→ 缺省 mixed 且 save 不外抛", () => {
    const saved = globalThis.localStorage;
    Object.defineProperty(globalThis, "localStorage", {
      value: {
        get length(): number {
          throw new Error("denied");
        },
        clear(): void {
          throw new Error("denied");
        },
        getItem(): string | null {
          throw new Error("denied");
        },
        key(): string | null {
          throw new Error("denied");
        },
        removeItem(): void {
          throw new Error("denied");
        },
        setItem(): void {
          throw new Error("denied");
        },
      },
      configurable: true,
    });
    expect(loadFavViewOrg()).toBe("mixed");
    expect(() => saveFavViewOrg("grouped")).not.toThrow();
    Object.defineProperty(globalThis, "localStorage", {
      value: saved,
      configurable: true,
      writable: true,
    });
  });
});

/// 测试尾清:不留记录,防用例间串场(缺省态用例依赖无记录)。
afterEach(() => {
  localStorage.clear();
});
