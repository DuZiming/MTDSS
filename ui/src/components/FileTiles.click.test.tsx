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

describe("选择圈退役(field r21 #1)+行 Ctrl 点选键语义", () => {
  it("tile-check 零在册;行 Ctrl 点选回传行身份键(复合/缺省同面)", async () => {
    const props = baseProps();
    props.rowKey = (e) => `dev-9\u0000${e.path}`;
    const host = await mountTiles(props);
    expect(document.querySelectorAll("button.tile-check").length).toBe(0);
    const row = [...document.querySelectorAll<HTMLElement>("[data-row-path]")].find(
      (el) => el.getAttribute("data-row-path") === "dev-9\u0000C:\\d\\笔记.txt",
    );
    expect(row).not.toBeUndefined();
    await act(async () => {
      row?.dispatchEvent(
        new MouseEvent("click", { bubbles: true, cancelable: true, ctrlKey: true }),
      );
    });
    expect(onRowClickMock).toHaveBeenCalledTimes(1);
    const [key, info] = onRowClickMock.mock.calls[0] as [
      string,
      { ctrlKey: boolean; shiftKey: boolean; metaKey: boolean },
    ];
    expect(key).toBe("dev-9\u0000C:\\d\\笔记.txt");
    expect(info.ctrlKey).toBe(true);
    host.remove();
  });
});
