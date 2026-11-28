/// 平铺视图重做(V4-1 T19)SSR 形状钉面:图标槽+两行文字、尺寸档、
/// 悬停浮出钮(选择圈/快捷下载)与零回归锚。node 环境 renderToStaticMarkup
/// (FileList.test 同手法;i18n 经 ../i18n 同步初始化)。交互(多选/双击/
/// 右键/空白清选)保持既有 DirViewProps 合同不另测(SSR 量不了事件;
/// FileList.test 三视图同构锚覆盖行身份键面)。
import { createElement } from "react";
import { readFileSync } from "node:fs";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it } from "vitest";

import "../i18n";

import type { FsEntry } from "../lib/browse";
import { fmtMtime, fmtSize } from "../lib/browse";
import { fileTypeName } from "../lib/filetype";
import FileTiles, { tileMetaLine } from "./FileTiles";
import type { DirViewProps } from "./FileList";

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

const entry = (over: Partial<FsEntry>): FsEntry => ({
  name: "横版照片.jpg",
  path: "C:\\pic\\横版照片.jpg",
  size: 240000,
  mtime_ms: 1760000000000,
  is_dir: false,
  ...over,
});

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

beforeEach(() => localStorage.clear());

/// 目录+文件两行(悬停钮/零回归共用)。
const rows = [
  entry({ name: "Screenshots", is_dir: true, path: "C:\\s" }),
  entry({}),
];

describe("tile 两行文字(V4-1 T19 mockup 钉面)", () => {
  it("文件行第二行 =「类型 · 大小」(fileTypeName · fmtSize 既有先例)", () => {
    expect(tileMetaLine(entry({}))).toBe(`图片 · ${fmtSize(240000, false)}`);
    expect(tileMetaLine(entry({}))).toBe("图片 · 234.4 KB");
  });

  it("目录行第二行 = 修改日期(fmtMtime 先例;「N 项」无数据源,V3-6b 起桌面不计数)", () => {
    const dir = entry({ name: "Screenshots", is_dir: true, path: "C:\\Screenshots" });
    expect(tileMetaLine(dir)).toBe(fmtMtime(1760000000000));
    expect(tileMetaLine(dir)).not.toContain("项");
  });

  it("无扩展名=「文件」段照拼;mtime=0 目录第二行空串(空段剔除不落悬空 ·)", () => {
    expect(tileMetaLine(entry({ name: "无类型", size: 12 }))).toBe("文件 · 12 B");
    expect(tileMetaLine(entry({ name: "d", is_dir: true, mtime_ms: 0 }))).toBe("");
  });

  it("渲染面:tile-name 与 tile-meta 两行都在,词面与纯函数一致", () => {
    const props = baseProps();
    props.rows = [entry({ name: "说明书.pdf", size: 8607181 })];
    const html = renderToStaticMarkup(createElement(FileTiles, props));
    expect(html).toContain('class="tile-name"');
    expect(html).toContain("说明书.pdf");
    expect(html).toContain('class="tile-meta"');
    expect(html).toContain(`PDF 文档 · ${fmtSize(8607181, false)}`);
    expect(html).toContain("说明书.pdf");
    expect(html).toContain(fileTypeName("说明书.pdf", false));
  });
});

describe("tile 图标槽(V4-1 T19 mockup 钉面)", () => {
  it("槽锚 .tile-slot 在;目录=folder 存档笔画,文件=页形存档笔画(icons 原样放大)", () => {
    const props = baseProps();
    props.rows = [
      entry({ name: "Screenshots", is_dir: true, path: "C:\\s" }),
      entry({}),
    ];
    const html = renderToStaticMarkup(createElement(FileTiles, props));
    expect((html.match(/tile-slot/g) ?? []).length).toBe(2);
    expect(html).toContain("M1.6 4.3"); // IcFolder 存档路径
    expect(html).toContain("M3.4 1.6"); // FilePage 存档路径
  });

  it("图/视频行默认仍图标打底(缩略图装载是客户端面,SSR 无 effects)", () => {
    const props = baseProps();
    props.rows = [entry({ name: "IMG_2041.mp4" }), entry({ name: "长截图.png" })];
    props.thumbDevices = ["dev-1", "dev-1"];
    const html = renderToStaticMarkup(createElement(FileTiles, props));
    expect(html).toContain("M3.4 1.6");
    expect(html).not.toContain("<img");
  });
});

describe("时长角标(V4-1 T21,mockup 角标=理想态;磁贴面のみ,列表行不动)", () => {
  it("duration_secs>0 文件行右下浮层 mm:ss/h:mm:ss(fmtDuration 单点)", () => {
    const props = baseProps();
    props.rows = [entry({ name: "影片.mp4", duration_secs: 3723 })];
    const html = renderToStaticMarkup(createElement(FileTiles, props));
    expect(html).toContain('class="tile-duration"');
    expect(html).toContain("1:02:03");
  });

  it("未知(缺键/0)= 零元素(有则显无则不显);目录行恒不显(即使误携)", () => {
    const props = baseProps();
    props.rows = [
      entry({ name: "影片2.mp4" }),
      entry({ name: "影片3.mp4", duration_secs: 0 }),
      entry({ name: "D", is_dir: true, path: "C:\d", duration_secs: 99 }),
    ];
    const html = renderToStaticMarkup(createElement(FileTiles, props));
    expect(html).not.toContain("tile-duration");
    expect(html).not.toContain("1:39");
  });
});

describe("三档尺寸(data-tile-size 落 CSS 变量)", () => {
  it("缺省 m 档;prop 直落档位属性", () => {
    const props = baseProps();
    props.rows = [entry({})];
    expect(renderToStaticMarkup(createElement(FileTiles, props))).toContain('data-tile-size="m"');
    props.tileSize = "l";
    expect(renderToStaticMarkup(createElement(FileTiles, props))).toContain('data-tile-size="l"');
    props.tileSize = "s";
    expect(renderToStaticMarkup(createElement(FileTiles, props))).toContain('data-tile-size="s"');
  });

  it("CSS 契约(App.css):三档变量规则在;槽 aspect-1 不裁切;图标尺寸随档", () => {
    const css = readFileSync(new URL("../App.css", import.meta.url), "utf8");
    // s/l 两档以落档选择器承载;m 档=根上缺省变量本身(存档 v5 面 118/78/66)
    for (const size of ["s", "l"]) {
      expect(css).toContain(`.file-tiles[data-tile-size="${size}"]`);
    }
    expect(css).toMatch(/\.file-tiles\s*\{[^}]*--tile-min:\s*118px[^}]*\}/);
    expect(css).toMatch(/\.file-tiles\s*\{[^}]*--tile-icon-dir:\s*78px[^}]*\}/);
    expect(css).toMatch(/\.file-tiles\s*\{[^}]*--tile-icon-file:\s*66px[^}]*\}/);
    expect(css).toMatch(/\.tile-slot\s*\{[^}]*aspect-ratio:\s*1[^}]*\}/);
    expect(css).toMatch(/\.tile-slot img\s*\{[^}]*object-fit:\s*contain[^}]*\}/);
    // 目录/文件两形态图标尺寸变量消费(78/66 存档 m 档锚)
    expect(css).toMatch(/\.tile\.dir \.tile-slot svg\s*\{[^}]*var\(--tile-icon-dir\)[^}]*\}/);
    expect(css).toMatch(/\.tile\.file \.tile-slot svg\s*\{[^}]*var\(--tile-icon-file\)[^}]*\}/);
  });
});

describe("悬停浮出钮(选择圈/快捷下载)", () => {
  it("选择圈每行一枚(CSS 悬停/选中浮出;词面=zh 新键)", () => {
    const props = baseProps();
    props.rows = rows;
    const html = renderToStaticMarkup(createElement(FileTiles, props));
    expect(html.match(/tile-check/g)).toBeNull(); // 选择圈已撤(field r21 #1)
    expect(html).not.toContain("切换选中"); // 选择圈已撤(field r21 #1)
  });

  it("快捷下载:处理器在场→文件行一枚目录零;缺省→零(零回归)", () => {
    const withHandler = baseProps();
    withHandler.rows = rows;
    withHandler.onQuickDownload = noop;
    const html = renderToStaticMarkup(createElement(FileTiles, withHandler));
    expect((html.match(/tile-download/g) ?? []).length).toBe(1);
    expect(html).toContain("下载到本机");
    const without = baseProps();
    without.rows = rows;
    const html2 = renderToStaticMarkup(createElement(FileTiles, without));
    expect(html2).not.toContain("tile-download");
  });

  it("CSS 契约(App.css):浮出钮默认隐,悬停/选中浮出(选中圈恒显)", () => {
    const css = readFileSync(new URL("../App.css", import.meta.url), "utf8");
    expect(css).not.toContain(".tile-check"); // 选择圈已撤(field r21 #1)
    expect(css).toMatch(/\.tile-download\s*\{[^}]*display:\s*none[^}]*\}/);
    expect(css).toMatch(/\.tile:hover \.tile-download\s*\{[^}]*display:\s*flex[^}]*\}/);
  });
});

describe("零回归锚(行交互合同不动)", () => {
  it("行身份/状态类原样:data-row-path、dir/file、selected/shift-range/cut/drag-source", () => {
    const props = baseProps();
    props.rows = rows;
    props.selected = new Set([rows[1].path]);
    props.rangeSet = new Set([rows[1].path]);
    props.cutPaths = new Set([rows[1].path]);
    props.dragSourcePaths = new Set([rows[1].path]);
    const html = renderToStaticMarkup(createElement(FileTiles, props));
    expect((html.match(/data-row-path/g) ?? []).length).toBe(2);
    expect(html).toContain('data-row-path="C:\\pic\\横版照片.jpg"');
    expect(html).toContain("tile file selected shift-range cut drag-source");
    expect(html).toContain("tile dir");
  });

  it("title 仍类型词(fileTypeName 同源)", () => {
    const props = baseProps();
    props.rows = [entry({})];
    const html = renderToStaticMarkup(createElement(FileTiles, props));
    expect(html).toContain('title="图片"');
  });

  it("状态块仍挂 DirStates(空态词面)", () => {
    const props = baseProps();
    props.rows = [];
    props.listing = { truncated: false };
    const html = renderToStaticMarkup(createElement(FileTiles, props));
    expect(html).toContain("此文件夹为空");
  });
});
