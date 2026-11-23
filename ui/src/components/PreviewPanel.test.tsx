// @vitest-environment happy-dom
/// 详情栏(field 定稿 2026-11-23 钉面:右侧栏=缩略图+元数据,预览主体=
/// ⧉ 弹窗):open 形(远端/本机)/工具栏(‹›/✕/⧉/下载门)/详情行(名字/
/// 大小/格式/三时间,a/c 缺=—)/Esc 关/换源收口(previewClose)/open 拒错误
/// 态/宽度 kv。daemon 依赖 vi.mock;拖调走 MouseEvent 形 pointer 事件。
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import "../i18n";

import type { FsEntry } from "../lib/browse";
import PreviewPanel, { type PreviewSource } from "./PreviewPanel";

const previewOpenMock = vi.fn();
const previewCloseMock = vi.fn();
const kvGetMock = vi.fn();
const kvPutMock = vi.fn();
const mountCoverMock = vi.fn();

vi.mock("../lib/daemon", () => ({
  previewOpen: (...args: unknown[]) => previewOpenMock(...args),
  previewClose: (...args: unknown[]) => previewCloseMock(...args),
  kvGet: (...args: unknown[]) => kvGetMock(...args),
  kvPut: (...args: unknown[]) => kvPutMock(...args),
}));
vi.mock("@mtdss/previewer", () => ({
  mountCover: (...args: unknown[]) => mountCoverMock(...args),
  COVER_OFFICE_MAX_BYTES: 25 * 1024 * 1024,
}));

beforeEach(() => {
  previewOpenMock.mockReset().mockResolvedValue({ handle: "h-1", port: 45678, token: "tok" });
  previewCloseMock.mockReset().mockResolvedValue(undefined);
  kvGetMock.mockReset().mockResolvedValue(null);
  kvPutMock.mockReset().mockResolvedValue(undefined);
  mountCoverMock.mockReset().mockReturnValue({ destroy: vi.fn() });
});

const entry = (over: Partial<FsEntry> = {}): FsEntry => ({
  name: "横版照片.jpg",
  path: "C:\\相册\\横版照片.jpg",
  size: 3300000,
  mtime_ms: 1760000000000,
  is_dir: false,
  ctime_ms: 1759000000000,
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
  it("远端:open 形 + 详情行(名字/大小/格式/三时间;缩略图 URL 携 t=)", async () => {
    await mountPanel();
    expect(previewOpenMock).toHaveBeenCalledWith("dev-9", entry().path, entry().mtime_ms, entry().size);
    const rows = Array.from(host!.querySelectorAll(".preview-details-row"));
    expect(rows.length).toBe(6);
    expect(rows[0].textContent).toContain("横版照片.jpg");
    expect(rows[1].textContent).toContain("MB");
    expect(rows[2].textContent).toContain("图片");
    expect(rows[4].textContent).toContain("2025-");
    expect(rows[5].textContent).toContain("—");
    // 静态封面(r13 #4):mountCover 收 buildPreviewRequest 同源载荷
    // (kind 路由 + thumb URL 携 ?t=)
    expect(mountCoverMock).toHaveBeenCalledTimes(1);
    const coverHost = mountCoverMock.mock.calls[0][0] as HTMLElement;
    expect(coverHost.className).toContain("preview-details-cover");
    const coverReq = mountCoverMock.mock.calls[0][1] as {
      kind: string;
      urls: { thumb?: string };
    };
    expect(coverReq.kind).toBe("image");
    expect(coverReq.urls.thumb).toContain("http://127.0.0.1:45678/f/h-1/thumb");
    expect(coverReq.urls.thumb).toContain("t=tok");
    expect(q(".preview-name").textContent).toContain("横版照片.jpg");
  });

  it("本机:device=null 直读臂 + 工具栏 ⧉ 上抛弹窗(field:底部 CTA 已撤)", async () => {
    const onPopOut = vi.fn();
    const props = baseProps();
    props.onPopOut = onPopOut;
    props.source = source({ device: null, entry: entry({ name: "本地.txt", path: "C:\本地.txt" }) });
    await mountPanel(props);
    expect(previewOpenMock).toHaveBeenCalledWith(null, "C:\本地.txt", entry().mtime_ms, entry().size);
    expect(Array.from(host!.querySelectorAll(".preview-details-row")).length).toBe(6);
    q(".preview-popout").click();
    expect(onPopOut).toHaveBeenCalledTimes(1);
    expect(host!.querySelector(".preview-details-open")).toBeNull(); // CTA 已撤
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

  it("r15 #4:⤓/↗ 两钮退役;⧉ 上抛弹出动作", async () => {
    const onPopOut = vi.fn();
    const props = baseProps();
    props.onPopOut = onPopOut;
    await mountPanel(props);
    expect(host!.querySelector(".preview-download")).toBeNull();
    expect(host!.querySelector(".preview-openext")).toBeNull();
    q(".preview-popout").click();
    expect(onPopOut).toHaveBeenCalledTimes(1);
  });

  it("r15 #6:文件夹源=零 preview_open 零 cover,信息面(文件夹图标+文件夹名词行)+⧉ 隐藏", async () => {
    await mountPanel({
      ...baseProps(),
      source: source({ entry: entry({ name: "相册", path: "C:相册", is_dir: true }) }),
    });
    expect(previewOpenMock).not.toHaveBeenCalled();
    expect(mountCoverMock).not.toHaveBeenCalled();
    expect(host!.querySelector(".preview-details-appicon svg")).not.toBeNull();
    const rows = Array.from(host!.querySelectorAll(".preview-details-row"));
    expect(rows[0]!.textContent).toContain("相册");
    expect(rows[0]!.textContent).toContain("文件夹名");
    expect(rows[2]!.textContent).toContain("文件夹");
    expect(host!.querySelector(".preview-popout")).toBeNull();
  });
});

describe("换源与收口(句柄引用计数不泄漏)", () => {
  it("换源:旧句柄 close,新源重新 open", async () => {
    const props = baseProps();
    await mountPanel(props);
    const next = baseProps();
    next.source = source({ entry: entry({ name: "长截图.png", path: "C:\相册\长截图.png" }) });
    await rerender(next);
    expect(previewCloseMock).toHaveBeenCalledWith("h-1");
    expect(previewOpenMock).toHaveBeenLastCalledWith("dev-9", "C:\相册\长截图.png", entry().mtime_ms, entry().size);
  });

  it("卸载:close 收口", async () => {
    await mountPanel();
    await unmount();
    expect(previewCloseMock).toHaveBeenCalledWith("h-1");
  });

  it("open 拒(源打开失败)→ 错误态,零详情", async () => {
    previewOpenMock.mockRejectedValue(new Error("源打开失败"));
    await mountPanel();
    expect(host!.querySelector(".preview-details-row")).toBeNull();
    expect(q(".preview-failed").textContent).toContain("无法打开预览");
    await unmount();
    expect(previewCloseMock).not.toHaveBeenCalled();
  });

});

describe("宽度 kv 持久(键 previewpanel:width)", () => {
  it("装载记忆:kvGet 380 → 面板宽 380px;旧%残留(55)→缺省 300px", async () => {
    kvGetMock.mockResolvedValue("380");
    await mountPanel();
    const aside = host!.querySelector(".preview-panel") as HTMLElement;
    expect(aside.style.width).toBe("380px");
    expect(kvGetMock).toHaveBeenCalledWith("previewpanel:width");
    await unmount();
    kvGetMock.mockResolvedValue("55"); // 旧百分比残留=脏值
    host = null;
    await mountPanel();
    const aside2 = host!.querySelector(".preview-panel") as HTMLElement;
    expect(aside2.style.width).toBe("300px");
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
    // 拖调即时面:右缘锚定 1000 − 光标 300 = 700 → clamp 上限 560
    expect(panel.style.width).toBe("560px");
    act(() => {
      grip.dispatchEvent(new MouseEvent("pointerup", { bubbles: true, clientX: 300 }));
    });
    await Promise.resolve();
    await act(async () => {});
    expect(kvPutMock).toHaveBeenCalledWith("previewpanel:width", "560");
  });
});

describe("图标类封面预门(field r14 #3:主界面同款 EntryIcon 对齐)", () => {
  it("不支持类型:零 mountCover,渲染 .preview-details-appicon", async () => {
    await mountPanel({
      ...baseProps(),
      source: source({ entry: entry({ name: "固件.bin", path: "C:\固件.bin" }) }),
    });
    expect(mountCoverMock).not.toHaveBeenCalled();
    expect(host!.querySelector(".preview-details-appicon svg")).not.toBeNull();
  });

  it("音频类型:同门(图标,不挂 cover);图片类型不受影响(照挂 cover)", async () => {
    await mountPanel({
      ...baseProps(),
      source: source({ entry: entry({ name: "歌.mp3", path: "C:\歌.mp3" }) }),
    });
    expect(mountCoverMock).not.toHaveBeenCalled();
    expect(host!.querySelector(".preview-details-appicon svg")).not.toBeNull();
    await mountPanel();
    expect(mountCoverMock).toHaveBeenCalledTimes(1); // jpg 走 cover
  });

  it("超门 office(>25MiB)走图标;同型小文件走 cover 裁剪呈现", async () => {
    await mountPanel({
      ...baseProps(),
      source: source({
        entry: entry({ name: "巨.pptx", path: "C:\巨.pptx", size: 26 * 1024 * 1024 }),
      }),
    });
    expect(mountCoverMock).not.toHaveBeenCalled();
    expect(host!.querySelector(".preview-details-appicon svg")).not.toBeNull();
    await mountPanel({
      ...baseProps(),
      source: source({ entry: entry({ name: "小.pptx", path: "C:\小.pptx", size: 4096 }) }),
    });
    expect(mountCoverMock).toHaveBeenCalledTimes(1);
  });
});
