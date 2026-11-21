// @vitest-environment happy-dom
/// EntryThumb(V4-1 T19 行内缩略图)挂载面单测:装载/回退态机。
/// happy-dom 门(主包 vitest 无 config 默认 node 环境,office.test 同款);
/// daemon 依赖 vi.mock(stub),fetch 全局覆写,IntersectionObserver 走
/// ioCtor 注入缝(假观察器,环境无布局引擎不会真触交)。
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const previewOpenMock = vi.fn();
const previewCloseMock = vi.fn();

vi.mock("../lib/daemon", () => ({
  previewOpen: (...args: unknown[]) => previewOpenMock(...args),
  previewClose: (...args: unknown[]) => previewCloseMock(...args),
}));

import type { FsEntry } from "../lib/browse";
import EntryThumb from "./EntryThumb";

const entryOf = (over: Partial<FsEntry> = {}): FsEntry => ({
  name: "照片.jpg",
  path: "C:\\pic\\照片.jpg",
  size: 123456,
  mtime_ms: 1760000000000,
  is_dir: false,
  ...over,
});

/// 假观察器:构造即按 fireImmediately 交一次判定(懒加载门的两面分别钉)。
let ioFire = true;
class FakeIO {
  private cb: IntersectionObserverCallback;
  constructor(cb: IntersectionObserverCallback) {
    this.cb = cb;
  }
  observe(): void {
    if (ioFire) {
      this.cb([{ isIntersecting: true } as IntersectionObserverEntry], this as unknown as IntersectionObserver);
    }
  }
  disconnect(): void {}
  unobserve(): void {}
}

/// fetch 覆写桩(记调用形;逐例重置)。
const fetchMock = vi.fn();

beforeEach(() => {
  previewOpenMock.mockReset();
  previewCloseMock.mockReset().mockResolvedValue(undefined); // 桥面恒 Promise
  fetchMock.mockReset();
  ioFire = true;
  Object.defineProperty(URL, "createObjectURL", {
    value: () => `blob:u-${Math.random()}`,
    configurable: true,
  });
  Object.defineProperty(URL, "revokeObjectURL", {
    value: () => {},
    configurable: true,
  });
  globalThis.fetch = fetchMock as unknown as typeof fetch;
});

afterEach(() => {
  vi.restoreAllMocks();
});

/// 挂载助手:happy-dom 容器 + createRoot + act 包裹微任务清排。
async function mountThumb(props: {
  entry: FsEntry;
  device: string | null;
  w?: number;
  ioCtor?: typeof IntersectionObserver;
}): Promise<{ host: HTMLElement; unmount: () => Promise<void> }> {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root: Root = createRoot(host);
  await act(async () => {
    root.render(
      createElement(EntryThumb, {
        w: 256,
        ioCtor: FakeIO as unknown as typeof IntersectionObserver,
        ...props,
      }),
    );
  });
  return {
    host,
    unmount: async () => {
      await act(async () => {
        root.unmount();
      });
      host.remove();
    },
  };
}

/// 清排挂载期微任务链(previewOpen→fetch→setUrl 全 Promise 面)。
const flush = async (): Promise<void> => {
  for (let i = 0; i < 10; i++) await Promise.resolve();
};

const OPEN_INFO = { handle: "h1", port: 47611, token: "tok" };

describe("EntryThumb 装载面(回环 thumb)", () => {
  it("图/视频行:preview_open(device,path,mtime,size)→ fetch Bearer thumb URL → img[blob:] + close 收口", async () => {
    previewOpenMock.mockResolvedValue(OPEN_INFO);
    fetchMock.mockResolvedValue(new Response(new Uint8Array([1, 2, 3]), { status: 200 }));
    const { host, unmount } = await mountThumb({ entry: entryOf(), device: "dev-9" });
    await act(async () => {
      await flush();
    });
    expect(previewOpenMock).toHaveBeenCalledWith("dev-9", "C:\\pic\\照片.jpg", 1760000000000, 123456);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe(`http://127.0.0.1:${OPEN_INFO.port}/f/${OPEN_INFO.handle}/thumb?w=256`);
    expect((init.headers as Record<string, string>).Authorization).toBe(`Bearer ${OPEN_INFO.token}`);
    const img = host.querySelector("img");
    expect(img).not.toBeNull();
    expect(img?.getAttribute("src")).toMatch(/^blob:/);
    expect(previewCloseMock).toHaveBeenCalledWith(OPEN_INFO.handle);
    await unmount();
  });

  it("宽参数随调用方(w=384 大档)进 URL", async () => {
    previewOpenMock.mockResolvedValue(OPEN_INFO);
    fetchMock.mockResolvedValue(new Response(new Uint8Array([1]), { status: 200 }));
    const { unmount } = await mountThumb({
      entry: entryOf({ name: "clip.mp4" }),
      device: "dev-9",
      w: 384,
    });
    await act(async () => {
      await flush();
    });
    expect(fetchMock.mock.calls[0]?.[0]).toBe(`http://127.0.0.1:${OPEN_INFO.port}/f/${OPEN_INFO.handle}/thumb?w=384`);
    await unmount();
  });
});

describe("EntryThumb 回退面(失败/空/拒绝/onerror)", () => {
  it("类型不在图/视频(.txt)→ 恒图标,零 IPC 零 fetch", async () => {
    const { host, unmount } = await mountThumb({
      entry: entryOf({ name: "笔记.txt" }),
      device: "dev-9",
    });
    await act(async () => {
      await flush();
    });
    expect(previewOpenMock).not.toHaveBeenCalled();
    expect(fetchMock).not.toHaveBeenCalled();
    expect(host.querySelector("img")).toBeNull();
    expect(host.querySelector("svg")).not.toBeNull();
    await unmount();
  });

  it("目录行 → 恒图标,零 IPC(device 在场也不试)", async () => {
    const { host, unmount } = await mountThumb({
      entry: entryOf({ name: "相册", is_dir: true }),
      device: "dev-9",
    });
    await act(async () => {
      await flush();
    });
    expect(previewOpenMock).not.toHaveBeenCalled();
    expect(host.querySelector("svg")).not.toBeNull();
    await unmount();
  });

  it("device null(无设备面)→ 恒图标,零 IPC", async () => {
    const { host, unmount } = await mountThumb({ entry: entryOf(), device: null });
    await act(async () => {
      await flush();
    });
    expect(previewOpenMock).not.toHaveBeenCalled();
    expect(host.querySelector("svg")).not.toBeNull();
    await unmount();
  });

  it("fetch !ok(410 句柄过期)→ 回退图标,close 仍收口", async () => {
    previewOpenMock.mockResolvedValue(OPEN_INFO);
    fetchMock.mockResolvedValue(new Response("gone", { status: 410 }));
    const { host, unmount } = await mountThumb({ entry: entryOf(), device: "dev-9" });
    await act(async () => {
      await flush();
    });
    expect(host.querySelector("img")).toBeNull();
    expect(host.querySelector("svg")).not.toBeNull();
    expect(previewCloseMock).toHaveBeenCalledWith(OPEN_INFO.handle);
    await unmount();
  });

  it("200 空体(T7 无后端语义)→ 回退图标", async () => {
    previewOpenMock.mockResolvedValue(OPEN_INFO);
    fetchMock.mockResolvedValue(new Response(new Uint8Array(0), { status: 200 }));
    const { host, unmount } = await mountThumb({ entry: entryOf(), device: "dev-9" });
    await act(async () => {
      await flush();
    });
    expect(host.querySelector("img")).toBeNull();
    expect(host.querySelector("svg")).not.toBeNull();
    await unmount();
  });

  it("preview_open 拒(源打开失败)→ 回退图标,零 fetch", async () => {
    previewOpenMock.mockRejectedValue(new Error("code 1: 源打开失败"));
    const { host, unmount } = await mountThumb({ entry: entryOf(), device: "dev-9" });
    await act(async () => {
      await flush();
    });
    expect(fetchMock).not.toHaveBeenCalled();
    expect(host.querySelector("img")).toBeNull();
    expect(host.querySelector("svg")).not.toBeNull();
    await unmount();
  });

  it("img onError(CSP 未放行回环面,T21 前常态)→ 回退图标", async () => {
    previewOpenMock.mockResolvedValue(OPEN_INFO);
    fetchMock.mockResolvedValue(new Response(new Uint8Array([1]), { status: 200 }));
    const { host, unmount } = await mountThumb({ entry: entryOf(), device: "dev-9" });
    await act(async () => {
      await flush();
    });
    const img = host.querySelector("img");
    expect(img).not.toBeNull();
    await act(async () => {
      img?.dispatchEvent(new Event("error"));
      await flush();
    });
    expect(host.querySelector("img")).toBeNull();
    expect(host.querySelector("svg")).not.toBeNull();
    await unmount();
  });
});

describe("EntryThumb 懒加载门(IntersectionObserver)", () => {
  it("未进入视口附近(观察器不交)→ 不发起装载,图标打底", async () => {
    ioFire = false;
    previewOpenMock.mockResolvedValue(OPEN_INFO);
    const { host, unmount } = await mountThumb({ entry: entryOf(), device: "dev-9" });
    await act(async () => {
      await flush();
    });
    expect(previewOpenMock).not.toHaveBeenCalled();
    expect(fetchMock).not.toHaveBeenCalled();
    expect(host.querySelector("svg")).not.toBeNull();
    await unmount();
  });

  it("卸载在途:open 迟到仍收口 close(句柄不泄漏),不落 img", async () => {
    let resolveOpen!: (v: typeof OPEN_INFO) => void;
    previewOpenMock.mockReturnValue(
      new Promise<typeof OPEN_INFO>((res) => {
        resolveOpen = res;
      }),
    );
    const { host, unmount } = await mountThumb({ entry: entryOf(), device: "dev-9" });
    await act(async () => {
      await flush();
    });
    expect(host.querySelector("img")).toBeNull();
    await act(async () => {
      await unmount();
    });
    await act(async () => {
      resolveOpen(OPEN_INFO);
      await flush();
    });
    expect(previewCloseMock).toHaveBeenCalledWith(OPEN_INFO.handle);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
