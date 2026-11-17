/// 目录三视图可选 rowKey SSR 冒烟(V3-3 Task 7;§16.2 色点契约退场,沿
/// ApprovalModal.test.tsx 手法):rowKey 缺省 = entry.path——零回归锚钉死
/// 桌面既有形态;传入时行身份键落到 React key 与 data-row-path(快捷主视
/// 图跨设备同路径行据此区分)。sourceColor/来源色点已全量移除(§16.2 UI
/// 零消费),任一视图恒无色点词面。react-dom/server 纯字符串渲染,无需
/// DOM;i18n 经 ../i18n 同步初始化(details 表头/空态词面解析)。三视图
/// 同一 DirViewProps 合同,逐视图同构断言(单钉一视图会漏另两视图接线)。
import { createElement } from "react";
import { readFileSync } from "node:fs";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it } from "vitest";

import "../i18n";

import {
  COLWIDTH_STORAGE_KEY,
  DEFAULT_COL_WIDTHS,
} from "../lib/colwidths";
import type { FsEntry } from "../lib/browse";
import FileDetails, { type DetailsSection } from "./FileDetails";
import FileList, { type DirViewProps } from "./FileList";
import FileTiles from "./FileTiles";

/// 内存版 localStorage 桩(colwidths.test.ts 同款):vitest 默认 node 环境
/// 没有 localStorage;§15.4 列宽钉死用例需播种自定义列宽,挂同构实现,
/// 不为此引 jsdom。
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

const noop = () => {};

const views = [
  ["list", FileList],
  ["tiles", FileTiles],
  ["details", FileDetails],
] as const;

/// 两条同名同 path 的行(仅 size 区分供 rowKey 取舍):
/// 混合视图跨设备同路径行的最小复现。
const twin = (size: number): FsEntry => ({
  name: "x",
  path: "x",
  size,
  mtime_ms: 1760000000000,
  is_dir: false,
});

/// 全量 DirViewProps(与 App.tsx dirProps 同形;回调一律 noop),
/// 各例按需覆写。
const baseProps = (): DirViewProps => ({
  rows: [],
  loading: false,
  listing: null,
  error: null,
  selected: new Set<string>(),
  cutPaths: new Set<string>(),
  rangeSet: new Set<string>(),
  dragSourcePaths: null,
  searchActive: false,
  sortState: { key: "name", dir: "asc" },
  onSortKey: noop,
  onRowClick: noop,
  onOpenDir: noop,
  onClearSelect: noop,
  onRetry: noop,
  onRowContextMenu: noop,
  onBlankContextMenu: noop,
});

describe("三视图 rowKey(SSR 冒烟:行身份键;§16.2 色点契约退场)", () => {
  for (const [name, View] of views) {
    it(`${name}:rowKey 复合键→两行 data-row-path 各落各键(同 path 不撞)`, () => {
      const props = baseProps();
      props.rows = [twin(1), twin(2)];
      props.rowKey = (e) => `dev${e.size}\u0000${e.path}`;
      const html = renderToStaticMarkup(createElement(View, props));
      expect(html).toContain(`data-row-path="dev1\u0000x"`);
      expect(html).toContain(`data-row-path="dev2\u0000x"`);
    });

    it(`${name}:零色点锚(§16.2):任一行恒无 src-dot 词面`, () => {
      const props = baseProps();
      props.rows = [twin(1), twin(2)];
      props.rowKey = (e) => `dev${e.size}\u0000${e.path}`;
      const html = renderToStaticMarkup(createElement(View, props));
      expect(html).not.toContain("src-dot");
    });

    it(`${name}:rowKey 缺省→零回归锚(data-row-path=path,无色点)`, () => {
      const props = baseProps();
      props.rows = [twin(1), twin(2)];
      const html = renderToStaticMarkup(createElement(View, props));
      expect((html.match(/data-row-path="x"/g) ?? []).length).toBe(2);
      expect(html).not.toContain("src-dot");
    });
  }
});

/// §14.4(返工波 2)FileDetails 单表分组节序列 SSR:sections 传入时
/// 节标签行与节内行**同表交错**——表头全页恰一枚(分组模式列头计数=1
/// 的组件面锚);节标签行契约(headClassName/data-drop-loc/内容)与
/// 节内行渲染(renderRow 同源,data-row-path)保真;缺省(无
/// sections)平铺模式零回归。
describe("FileDetails §14.4(粘性列头/分组单表 SSR)", () => {
  const entry = (name: string, isDir = false): FsEntry => ({
    name,
    path: name,
    size: isDir ? 0 : 12,
    mtime_ms: 1760000000000,
    is_dir: isDir,
  });
  const sec = (key: string, rows: FsEntry[]): DetailsSection => ({
    key,
    headClassName: "fav-group-head",
    headDropLoc: `dev-1\u0000C:\\a`,
    head: createElement("strong", null, key),
    rows,
  });

  it("sections 模式:表头恰一枚(<thead 计数=1),节标签行与节内行同表交错", () => {
    const html = renderToStaticMarkup(
      createElement(FileDetails, {
        ...baseProps(),
        sections: [sec("s1", [entry("a"), entry("b", true)]), sec("s2", [entry("c")])],
      }),
    );
    expect((html.match(/<thead/g) ?? []).length).toBe(1);
    expect((html.match(/fav-group-head/g) ?? []).length).toBe(2);
    // 节级落点契约原样(T12 命中测试消费)
    expect(html).toContain(`data-drop-loc="dev-1\u0000C:\\a"`);
    // 节内行 renderRow 同源:三行 data-row-path
    expect((html.match(/data-row-path/g) ?? []).length).toBe(3);
    // 交错序:thead → s1 节头 → s1 两行 → s2 节头 → s2 一行
    const iHead = html.indexOf("<thead");
    const iS1 = html.indexOf("<strong>s1</strong>");
    const iA = html.indexOf('data-row-path="a"');
    const iB = html.indexOf('data-row-path="b"');
    const iS2 = html.indexOf("<strong>s2</strong>");
    const iC = html.indexOf('data-row-path="c"');
    expect(iHead).toBeLessThan(iS1);
    expect(iS1).toBeLessThan(iA);
    expect(iA).toBeLessThan(iB);
    expect(iB).toBeLessThan(iS2);
    expect(iS2).toBeLessThan(iC);
  });

  it("sections 模式:空节只留标签行,无行/无状态块(listing 缺省 null)", () => {
    const html = renderToStaticMarkup(
      createElement(FileDetails, { ...baseProps(), sections: [sec("s-empty", [])] }),
    );
    expect((html.match(/<thead/g) ?? []).length).toBe(1);
    expect(html).toContain("fav-group-head");
    expect(html).toContain("<strong>s-empty</strong>");
    expect(html).not.toContain("data-row-path");
    expect(html).not.toContain('class="state"'); // 空态/加载/错误块不渲染
  });

  it("缺省(无 sections):平铺模式零回归——表头恰一枚,rows 照常渲染", () => {
    const props = baseProps();
    props.rows = [entry("a"), entry("b", true)];
    const html = renderToStaticMarkup(createElement(FileDetails, props));
    expect((html.match(/<thead/g) ?? []).length).toBe(1);
    expect((html.match(/data-row-path/g) ?? []).length).toBe(2);
    expect(html).not.toContain("fav-group-head");
  });
});

/// §15.4(返工波 4)列宽钉死 SSR:四列表头**恒带显式宽**——名称未拖 =
/// calc(100% - 其余三列生效和)(既有弹性语义的显式化,逐像素等价),
/// 拖过 = 钉死 px;其余三列自定义优先、缺省走默认基线。行集增减(分组
/// 收起/展开、渐进到达)在 table-layout:fixed 下零列宽重协商;表头零
/// 位移另由 .file-table-wrap scrollbar-gutter:stable 恒保留纵滚槽保证
/// (App.css,SSR 量不了布局,机制断言锚「每 th 有宽」+「表 min-width
/// = 四列和(名称未钉按其下限计)」两个可序列化面;平铺与 sections 两
/// 渲染同构钉死——同一 thead,单钉一面会漏)。
describe("FileDetails §15.4(列宽钉死 SSR:四列恒显式宽)", () => {
  beforeEach(() => localStorage.clear());

  const entry = (name: string): FsEntry => ({
    name,
    path: name,
    size: 12,
    mtime_ms: 1760000000000,
    is_dir: false,
  });
  const sec = (key: string): DetailsSection => ({
    key,
    headClassName: "fav-group-head",
    head: createElement("strong", null, key),
    rows: [entry("a")],
  });

  /// 逐 th 提取 inline style 的 width 值(表头序 = name/mtime/type/size)。
  const thWidths = (html: string): string[] =>
    [...html.matchAll(/<th [^>]*>/g)].map(
      (m) =>
        /width:([^;"]+)/.exec(/style="([^"]*)"/.exec(m[0])?.[1] ?? "")?.[1] ??
        "",
    );
  const tableStyle = (html: string): string =>
    /<table class="file-table" style="([^"]*)"/.exec(html)?.[1] ?? "";

  /// 名称弹性显式宽:calc(100% - 其余三列默认基线之和)。
  const flex =
    "calc(100% - " +
    (DEFAULT_COL_WIDTHS.mtime + DEFAULT_COL_WIDTHS.type + DEFAULT_COL_WIDTHS.size) +
    "px)";

  it("平铺渲染:名称未拖 = calc 弹性显式宽,其余三列走默认基线;表 min-width=四列和", () => {
    const props = baseProps();
    props.rows = [entry("a")];
    const html = renderToStaticMarkup(createElement(FileDetails, props));
    expect(thWidths(html)).toEqual([flex, "170px", "130px", "90px"]);
    expect(tableStyle(html)).toBe("min-width:530px");
  });

  it("sections 渲染:同一 thead 同构——四列显式宽与平铺逐列一致", () => {
    const html = renderToStaticMarkup(
      createElement(FileDetails, {
        ...baseProps(),
        sections: [sec("s1")],
      }),
    );
    expect((html.match(/<thead/g) ?? []).length).toBe(1);
    expect(thWidths(html)).toEqual([flex, "170px", "130px", "90px"]);
    expect(tableStyle(html)).toBe("min-width:530px");
  });

  it("名称已钉(localStorage 播种):名称列钉死 px,表宽=四列和 inline(mode B)", () => {
    localStorage.setItem(COLWIDTH_STORAGE_KEY, JSON.stringify({ name: 260 }));
    const props = baseProps();
    props.rows = [entry("a")];
    const html = renderToStaticMarkup(createElement(FileDetails, props));
    expect(thWidths(html)).toEqual(["260px", "170px", "130px", "90px"]);
    expect(tableStyle(html)).toBe("width:650px;min-width:650px"); // 260+170+130+90
  });

  it("调谁动谁(播种 mtime 200):被拖列 px 落位,名称 calc 随其余三列和收放", () => {
    localStorage.setItem(COLWIDTH_STORAGE_KEY, JSON.stringify({ mtime: 200 }));
    const props = baseProps();
    props.rows = [entry("a")];
    const html = renderToStaticMarkup(createElement(FileDetails, props));
    expect(thWidths(html)).toEqual(["calc(100% - 420px)", "200px", "130px", "90px"]);
    expect(tableStyle(html)).toBe("min-width:560px"); // 名称未钉按其下限 140 计
  });
});

/// §5.1-3(v3p6)列表表头固定+悬停收敛 SSR:设备浏览列表(dir 位)对齐
/// V3-3 §14.4 已交付面——粘性表头+仅表头悬停高亮。SSR 量不了布局,断言
/// 锚取「渲染 DOM 类名 + App.css 同名规则」双面:.file-list-head 既是
/// DOM 锚也是粘性/悬停 CSS 的载体(粘性规则体内 position:sticky;悬停
/// 收敛=仅 .file-list-head:hover 亮 --hover,行悬停 .file-row:hover 原
/// 样不动)。showHead 缺省 false:FavView 逐节 list 仍无表头(V3-3 §14.4
/// 「list/tiles 无表头概念」裁定不动,零回归)。
describe("FileList §5.1-3(粘性表头+悬停收敛 SSR)", () => {
  const entry = (name: string, isDir = false): FsEntry => ({
    name,
    path: name,
    size: isDir ? 0 : 12,
    mtime_ms: 1760000000000,
    is_dir: isDir,
  });

  it("showHead:表头恰一枚(class=file-list-head),先于行,带名称词面", () => {
    const props = baseProps();
    props.rows = [entry("a"), entry("b", true)];
    props.showHead = true;
    const html = renderToStaticMarkup(createElement(FileList, props));
    expect((html.match(/file-list-head/g) ?? []).length).toBe(1);
    const iHead = html.indexOf('class="file-list-head"');
    const iRow = html.indexOf('data-row-path="a"');
    expect(iHead).toBeGreaterThanOrEqual(0);
    expect(iHead).toBeLessThan(iRow);
    expect(html).toContain("名称"); // files.column.name(zh 词面,与 details 表头同源)
    // 行面零回归:行类/交互属性照旧
    expect((html.match(/data-row-path/g) ?? []).length).toBe(2);
  });

  it("缺省(无 showHead):无表头——FavView 逐节 list 零回归", () => {
    const props = baseProps();
    props.rows = [entry("a")];
    const html = renderToStaticMarkup(createElement(FileList, props));
    expect(html).not.toContain("file-list-head");
    expect((html.match(/data-row-path/g) ?? []).length).toBe(1);
  });

  it("CSS 契约(App.css):.file-list-head 粘性规则在,悬停仅表头+行悬停不动", () => {
    const css = readFileSync(new URL("../App.css", import.meta.url), "utf8");
    // 粘性:.file-list-head 规则体内 position:sticky(V3-3 §14.4 同款承载)
    expect(css).toMatch(/\.file-list-head\s*\{[^}]*position:\s*sticky[^}]*\}/);
    // 悬停收敛:仅表头悬停规则存在(亮 --hover);行悬停规则原样仍在
    expect(css).toMatch(/\.file-list-head:hover\s*\{[^}]*background:\s*var\(--hover\)[^}]*\}/);
    expect(css).toMatch(/\.file-row:hover\s*\{[^}]*background:\s*var\(--hover\)[^}]*\}/);
  });
});
