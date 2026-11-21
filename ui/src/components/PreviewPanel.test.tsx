// @vitest-environment happy-dom
/// 预览面板(V4-1 T20)钉面:开面板 open 形(远端/本机两路)/工具栏动作
/// (‹›/✕/⧉/下载门)/Esc 关/换源收口(destroy+previewClose)/open 拒错误态
/// /宽度 kv 持久(装载+拖调)。previewer 与 daemon 依赖 vi.mock(挂载面,
/// EntryThumb.test 同炉);拖调走 MouseEvent 形 pointer 事件派发(合成事件
/// 无真实指针,组件内捕获照例 try/catch)。
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import "../i18n";

import type { FsEntry } from "../lib/browse";
import type { PreviewRequest } from "@mtdss/previewer";
import PreviewPanel, { type PreviewSource } from "./PreviewPanel";

const previewOpenMock = vi.fn();
const previewCloseMock = vi.fn();
const kvGetMock = vi.fn();
const kvPutMock = vi.fn();

vi.mock("../lib/daemon", () => ({
  previewOpen: (...args: unknown[]) => previewOpenMock(...args),
  previewClose: (...args: unknown[]) => previewCloseMock(...args),
  kvGet: (...args: unknown[]) => kvGetMock(...args),
  kvPut: (...args: unknown[]) => kvPutMock(...args),
}));

const destroyMock = vi.fn();
const mountPreviewMock = vi.fn();

vi.mock("@mtdss/previewer", () => ({
  mountPreview: (...args: unknown[]) => mountPreviewMock(...args),
}));

beforeEach(() => {
  previewOpenMock.mockReset().mockResolvedValue({ handle: "h-1", port: 45678, token: "tok" });
  previewCloseMock.mockReset().mockResolvedValue(undefined);
  kvGetMock.mockReset().mockResolvedValue(null);
  kvPutMock.mockReset().mockResolvedValue(undefined);
  mountPreviewMock
    .mockReset()
    .mockImplementation((_el: unknown, _req: unknown) => ({ destroy: destroyMock }));
  destroyMock.mockReset();
});

const entry = (over: Partial<FsEntry> = {}): FsEntry => ({
  name: "横版照片.jpg",
  path: "C:\\相册\\横版照片.jpg",
  size: 3300000,
  mtime_ms: 1760000000000,
  is_dir: false,
  ...over,
});

const source = (over: Partial<PreviewSource> = {}): PreviewSource => ({
  device: "dev-9",
  entry: entry(),
  ...over,
});

const noop = (): void => {};
const baseProps = () => ({
  source: source(),
  hasPrev: true,
  hasNext: true,
  onPrev: noop,
  onNext: noop,
  onClose: noop,
  onDownload: noop,
  canDownload: true,
  onOpenExternal: noop,
  onPopOut: noop,
});

let root: Root | null = null;
let host: HTMLElement | null = null;

async function mountPanel(props = baseProps()): Promise<HTMLElement> {
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  await act(async () => {
    root!.render(createElement(PreviewPanel, props));
  });
  return host;
}

async function rerender(props: ReturnType<typeof baseProps>): Promise<void> {
  await act(async () => {
    root!.render(createElement(PreviewPanel, props));
  });
}

async function unmount(): Promise<void> {
  await act(async () => {
    root?.unmount();
  });
  root = null;
  host?.remove();
  host = null;
}

afterEach(async () => {
  if (root != null) await unmount();
});

const q = (sel: string): HTMLElement => {
  const el = host!.querySelector(sel);
  if (el == null) throw new Error(`missing ${sel}`);
  return el as HTMLElement;
};

describe("开面板(preview_open 形)", () => {
  it("远端:open(device,path,mtime,size)→ mountPreview(req urls 携 ?t=)", async () => {
    await mountPanel();
    expect(previewOpenMock).toHaveBeenCalledWith("dev-9", entry().path, entry().mtime_ms, entry().size);
    expect(mountPreviewMock).toHaveBeenCalledTimes(1);
    const req = mountPreviewMock.mock.calls[0][1] as PreviewRequest;
    expect(req.kind).toBe("image");
    expect(req.urls.raw).toBe("http://127.0.0.1:45678/f/h-1/raw?t=tok");
    expect(req.urls.thumb).toBe("http://127.0.0.1:45678/f/h-1/thumb?t=tok&w=256");
    expect(req.meta.name).toBe("横版照片.jpg");
    // 文件名+大小上屏(mockup 工具栏钉面)
    expect(q(".preview-name").textContent).toContain("横版照片.jpg");
    expect(q(".preview-name").textContent).toContain("MB");
  });

  it("本机:device=null 直读臂", async () => {
    const props = baseProps();
    props.source = source({ device: null, entry: entry({ name: "本地.txt", path: "C:\\本地.txt" }) });
    await mountPanel(props);
    expect(previewOpenMock).toHaveBeenCalledWith(null, "C:\\本地.txt", entry().mtime_ms, entry().size);
    const req = mountPreviewMock.mock.calls[0][1] as PreviewRequest;
    expect(req.kind).toBe("text");
  });
});

describe("工具栏与关闭", () => {
  it("‹› 透传 onPrev/onNext;门控 disabled", async () => {
    const onPrev = vi.fn();
    const onNext = vi.fn();
    const props = baseProps();
    props.onPrev = onPrev;
    props.onNext = onNext;
    props.hasPrev = false;
    await mountPanel(props);
    (q(".preview-prev") as HTMLButtonElement).click();
    (q(".preview-next") as HTMLButtonElement).click();
    expect(onPrev).not.toHaveBeenCalled(); // disabled 吞点击
    expect(onNext).toHaveBeenCalledTimes(1);
    expect((q(".preview-prev") as HTMLButtonElement).disabled).toBe(true);
    expect((q(".preview-next") as HTMLButtonElement).disabled).toBe(false);
  });

  it("✕ 与 Esc 均关面板", async () => {
    const onClose = vi.fn();
    const props = baseProps();
    props.onClose = onClose;
    await mountPanel(props);
    q(".preview-close").click();
    expect(onClose).toHaveBeenCalledTimes(1);
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it("下载门:canDownload 才呈现下载钮;⧉ 上抛弹出动作", async () => {
    const onPopOut = vi.fn();
    const props = baseProps();
    props.onPopOut = onPopOut;
    await mountPanel(props);
    expect(q(".preview-download") != null).toBe(true);
    q(".preview-popout").click();
    expect(onPopOut).toHaveBeenCalledTimes(1);

    const local = baseProps();
    local.canDownload = false;
    await rerender(local);
    expect(host!.querySelector(".preview-download")).toBeNull();
  });

  it("用其他应用打开入口恒呈现,点击上抛", async () => {
    const onOpenExternal = vi.fn();
    const props = baseProps();
    props.onOpenExternal = onOpenExternal;
    await mountPanel(props);
    q(".preview-openext").click();
    expect(onOpenExternal).toHaveBeenCalledTimes(1);
  });
});

describe("换源与收口(句柄引用计数不泄漏)", () => {
  it("换源:旧装配 destroy+旧句柄 close,新源重新 open+mount", async () => {
    const props = baseProps();
    await mountPanel(props);
    const next = baseProps();
    next.source = source({ entry: entry({ name: "长截图.png", path: "C:\\相册\\长截图.png" }) });
    await rerender(next);
    expect(destroyMock).toHaveBeenCalledTimes(1);
    expect(previewCloseMock).toHaveBeenCalledWith("h-1");
    expect(previewOpenMock).toHaveBeenLastCalledWith("dev-9", "C:\\相册\\长截图.png", entry().mtime_ms, entry().size);
    expect(mountPreviewMock).toHaveBeenCalledTimes(2);
  });

  it("卸载:destroy+close 收口", async () => {
    await mountPanel();
    await unmount();
    expect(destroyMock).toHaveBeenCalledTimes(1);
    expect(previewCloseMock).toHaveBeenCalledWith("h-1");
  });

  it("open 拒(源打开失败)→ 错误态,零装配", async () => {
    previewOpenMock.mockRejectedValue(new Error("源打开失败"));
    await mountPanel();
    expect(mountPreviewMock).not.toHaveBeenCalled();
    expect(q(".preview-failed").textContent).toContain("无法打开预览");
    // 错误态关闭面板不误收口(无句柄在手)
    await unmount();
    expect(previewCloseMock).not.toHaveBeenCalled();
  });
});

describe("宽度 kv 持久(键 previewpanel:width)", () => {
  it("装载记忆:kvGet 55 → 面板宽 55%", async () => {
    kvGetMock.mockResolvedValue("55");
    await mountPanel();
    const aside = host!.querySelector(".preview-panel") as HTMLElement;
    expect(aside.style.width).toBe("55%");
    expect(kvGetMock).toHaveBeenCalledWith("previewpanel:width");
  });

  it("拖调:左缘拖柄移动即时改宽,松手 kvPut 落 clamp 整串", async () => {
    await mountPanel();
    const grip = q(".preview-grip");
    const panel = host!.querySelector(".preview-panel") as HTMLElement;
    // 宿主几何缝:面板宿主(挂载宿主 div)宽 1000、右缘 1000——拖柄几何
    // 读宿主 rect(无布局引擎,测试注入)。
    const parent = panel.parentElement as HTMLElement;
    parent.getBoundingClientRect = () =>
      ({ right: 1000, width: 1000, left: 0, top: 0, x: 0, y: 0 } as DOMRect);
    act(() => {
      grip.dispatchEvent(
        new MouseEvent("pointerdown", { bubbles: true, button: 0, clientX: 600 }),
      );
    });
    act(() => {
      grip.dispatchEvent(
        new MouseEvent("pointermove", { bubbles: true, clientX: 300 }),
      );
    });
    // 拖调即时面:宽度收进 [25,70] 区间(此处触上限 70)
    expect(panel.style.width).toBe("70%");
    act(() => {
      grip.dispatchEvent(new MouseEvent("pointerup", { bubbles: true, clientX: 300 }));
    });
    await Promise.resolve();
    await act(async () => {});
    expect(kvPutMock).toHaveBeenCalledWith("previewpanel:width", "70");
  });
});
