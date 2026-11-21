// @vitest-environment happy-dom
/// FileTiles 行交互钉面(fix round 1 Important #1):选择圈合成 Ctrl 切换
/// 必须携带**行身份键**——FavView 的复合键从事件宿主行 data-row-path 回取,
/// 合成对象无 currentTarget 会落空回退裸 path(fav 视图幽灵选择)。SSR
/// 量不了点击,走挂载面(EntryThumb.test 同炉;条目用 txt/目录,零缩略
/// 图 IPC 面,daemon.ts 仅导入不调用)。
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import "../i18n";

import type { FsEntry } from "../lib/browse";
import FileTiles from "./FileTiles";
import type { DirViewProps } from "./FileList";

const onRowClickMock = vi.fn();

afterEach(() => {
  onRowClickMock.mockReset();
});

const entry = (over: Partial<FsEntry> = {}): FsEntry => ({
  name: "笔记.txt",
  path: "C:\\d\\笔记.txt",
  size: 12,
  mtime_ms: 1760000000000,
  is_dir: false,
  ...over,
});

const baseProps = (): DirViewProps => ({
  rows: [entry()],
  loading: false,
  listing: null,
  error: null,
  selected: new Set<string>(),
  cutPaths: new Set<string>(),
  rangeSet: new Set<string>(),
  dragSourcePaths: null,
  searchActive: false,
  sortState: { key: "name", dir: "asc" },
  onSortKey: () => {},
  onRowClick: onRowClickMock,
  onOpenDir: () => {},
  onClearSelect: () => {},
  onRetry: () => {},
  onRowContextMenu: () => {},
  onBlankContextMenu: () => {},
});

async function mountTiles(props: DirViewProps): Promise<HTMLElement> {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root: Root = createRoot(host);
  await act(async () => {
    root.render(createElement(FileTiles, props));
  });
  return host;
}

/// 选择圈按钮 = .tile-check(hover 浮出但 DOM 恒在,直点即达)。行定位走
/// getAttribute 原值比对——复合键含 NUL/反斜杠,CSS 选择器串面转义不保真。
async function clickCheck(rowPath: string): Promise<void> {
  const row = [...document.querySelectorAll<HTMLElement>("[data-row-path]")].find(
    (el) => el.getAttribute("data-row-path") === rowPath,
  );
  expect(row).not.toBeUndefined();
  const check = row?.querySelector("button.tile-check");
  expect(check).not.toBeNull();
  await act(async () => {
    check?.dispatchEvent(
      new MouseEvent("click", { bubbles: true, cancelable: true, ctrlKey: true }),
    );
  });
}

describe("选择圈合成 Ctrl 切换携行身份键(fix round 1 #1)", () => {
  it("复合 rowKey(fav 混合视图):点击圈回传复合键,非裸 path", async () => {
    const props = baseProps();
    props.rowKey = (e) => `dev-9\u0000${e.path}`;
    const host = await mountTiles(props);
    await clickCheck("dev-9\u0000C:\\d\\笔记.txt");
    expect(onRowClickMock).toHaveBeenCalledTimes(1);
    const [key, info] = onRowClickMock.mock.calls[0] as [
      string,
      { ctrlKey: boolean; shiftKey: boolean; metaKey: boolean },
    ];
    expect(key).toBe("dev-9\u0000C:\\d\\笔记.txt");
    expect(info.ctrlKey).toBe(true);
    expect(info.shiftKey).toBe(false);
    host.remove();
  });

  it("缺省 rowKey(dir 位零回归):点击圈回传 path(≡键)", async () => {
    const host = await mountTiles(baseProps());
    await clickCheck("C:\\d\\笔记.txt");
    expect(onRowClickMock).toHaveBeenCalledTimes(1);
    expect(onRowClickMock.mock.calls[0]?.[0]).toBe("C:\\d\\笔记.txt");
    host.remove();
  });
});
