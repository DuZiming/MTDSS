// @vitest-environment happy-dom
/// 三视图文件行双击接线(V4-1 T20):目录行双击仍钻取(onOpenDir),文件
/// 行双击 = onOpenFile(开预览面板;处理器缺省零回归),多选修饰键双击
/// 两者皆不触发(既有守卫不动)。SSR 量不了双击,三视图逐一同构挂载
/// (FileTiles.click.test 同炉;条目 txt/目录,零缩略图 IPC 面)。
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import "../i18n";

import type { FsEntry } from "../lib/browse";
import FileDetails from "./FileDetails";
import FileList, { type DirViewProps } from "./FileList";
import FileTiles from "./FileTiles";

const file = (): FsEntry => ({
  name: "笔记.txt",
  path: "C:\\d\\笔记.txt",
  size: 12,
  mtime_ms: 1760000000000,
  is_dir: false,
});

const dir = (): FsEntry => ({
  name: "相册",
  path: "C:\\d\\相册",
  size: 0,
  mtime_ms: 1760000000000,
  is_dir: true,
});

const noop = (): void => {};

function baseProps(): DirViewProps {
  return {
    rows: [file(), dir()],
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
  };
}

let root: Root | null = null;

async function mountView(View: typeof FileList | typeof FileTiles | typeof FileDetails, props: DirViewProps): Promise<HTMLElement> {
  const host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  await act(async () => {
    root!.render(createElement(View, props));
  });
  return host;
}

afterEach(async () => {
  if (root != null) {
    await act(async () => {
      root!.unmount();
    });
    root = null;
  }
});

describe.each([
  ["list", FileList],
  ["tiles", FileTiles],
  ["details", FileDetails],
])("%s 双击接线(T20)", (_name, View) => {
  it("文件行双击 → onOpenFile;目录行双击 → onOpenDir(钻取不变)", async () => {
    const onOpenFile = vi.fn();
    const onOpenDir = vi.fn();
    const props = baseProps();
    props.onOpenFile = onOpenFile;
    props.onOpenDir = onOpenDir;
    const host = await mountView(View, props);
    const rows = host.querySelectorAll<HTMLElement>("[data-row-path]");
    expect(rows.length).toBe(2);
    act(() => {
      rows[0].dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    });
    expect(onOpenFile).toHaveBeenCalledTimes(1);
    expect(onOpenFile.mock.calls[0][0]).toMatchObject({ path: "C:\\d\\笔记.txt" });
    expect(onOpenDir).not.toHaveBeenCalled();
    act(() => {
      rows[1].dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    });
    expect(onOpenDir).toHaveBeenCalledTimes(1);
    expect(onOpenDir.mock.calls[0][0]).toMatchObject({ path: "C:\\d\\相册" });
    expect(onOpenFile).toHaveBeenCalledTimes(1);
  });

  it("多选修饰键双击两皆不触发(既有守卫);处理器缺省零回归不抛", async () => {
    const onOpenFile = vi.fn();
    const onOpenDir = vi.fn();
    const props = baseProps();
    props.onOpenFile = onOpenFile;
    props.onOpenDir = onOpenDir;
    const host = await mountView(View, props);
    const rows = host.querySelectorAll<HTMLElement>("[data-row-path]");
    act(() => {
      rows[0].dispatchEvent(
        new MouseEvent("dblclick", { bubbles: true, ctrlKey: true }),
      );
    });
    expect(onOpenFile).not.toHaveBeenCalled();
    expect(onOpenDir).not.toHaveBeenCalled();

    // 缺省(不挂处理器)文件行双击零回归:无动作不抛
    const bare = baseProps();
    const host2 = document.createElement("div");
    document.body.appendChild(host2);
    const root2 = createRoot(host2);
    await act(async () => {
      root2.render(createElement(View, bare));
    });
    await act(async () => {
      host2.querySelectorAll<HTMLElement>("[data-row-path]")[0].dispatchEvent(
        new MouseEvent("dblclick", { bubbles: true }),
      );
    });
    await act(async () => {
      root2.unmount();
    });
    host2.remove();
  });
});
