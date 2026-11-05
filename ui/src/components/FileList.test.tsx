/// 目录三视图可选 rowKey/sourceColor SSR 冒烟(V3-3 Task 7,沿
/// ApprovalModal.test.tsx 手法):rowKey 缺省 = entry.path、sourceColor
/// 缺省不渲染——零回归锚钉死桌面既有形态;传入时行身份键落到 React
/// key 与 data-row-path(混合视图跨设备同路径行据此区分),色点仅落
/// 非 undefined 行。react-dom/server 纯字符串渲染,无需 DOM;i18n 经
/// ../i18n 同步初始化(details 表头/空态词面解析)。三视图同一
/// DirViewProps 合同,逐视图同构断言(单钉一视图会漏另两视图接线)。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import "../i18n";

import type { FsEntry } from "../lib/browse";
import FileDetails from "./FileDetails";
import FileList, { type DirViewProps } from "./FileList";
import FileTiles from "./FileTiles";

const noop = () => {};

const views = [
  ["list", FileList],
  ["tiles", FileTiles],
  ["details", FileDetails],
] as const;

/// 两条同名同 path 的行(仅 size 区分供 rowKey/sourceColor 取舍):
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

describe("三视图 rowKey/sourceColor(SSR 冒烟:行身份键与来源色点)", () => {
  for (const [name, View] of views) {
    it(`${name}:rowKey 复合键→两行 data-row-path 各落各键(同 path 不撞)`, () => {
      const props = baseProps();
      props.rows = [twin(1), twin(2)];
      props.rowKey = (e) => `dev${e.size}\u0000${e.path}`;
      const html = renderToStaticMarkup(createElement(View, props));
      expect(html).toContain(`data-row-path="dev1\u0000x"`);
      expect(html).toContain(`data-row-path="dev2\u0000x"`);
    });

    it(`${name}:sourceColor→仅非 undefined 行有色点,背景值钉死`, () => {
      const props = baseProps();
      props.rows = [twin(1), twin(2)];
      props.sourceColor = (e) => (e.size === 1 ? "#8B5CF6" : undefined);
      const html = renderToStaticMarkup(createElement(View, props));
      expect(html).toContain(`class="src-dot"`);
      expect(html).toContain(`background:#8B5CF6`);
      expect((html.match(/src-dot/g) ?? []).length).toBe(1);
    });

    it(`${name}:两 prop 缺省→零回归锚(data-row-path=path,无色点)`, () => {
      const props = baseProps();
      props.rows = [twin(1), twin(2)];
      const html = renderToStaticMarkup(createElement(View, props));
      expect((html.match(/data-row-path="x"/g) ?? []).length).toBe(2);
      expect(html).not.toContain("src-dot");
    });
  }
});
