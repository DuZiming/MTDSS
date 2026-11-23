// @vitest-environment happy-dom
/// 静态封面钉面(field r13 #4「右侧栏只展示静态内容」):按 kind 分发——
/// image/video=<img>(thumb 面)、audio/unsupported=图标、text/md=前段文本
/// (Range 头)、pdf=pdf.js 首页 canvas、office=裁剪挂载(尺寸门)。全分支
/// 尽力而为:失败面=文件图标(不 throw)。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("pdfjs-dist", async () => {
  const fake = await import("./pdfjs-fake");
  return fake.makeFakePdfjs();
});
vi.mock("pdfjs-dist/build/pdf.worker.min.mjs", () => ({}));

/// router 桩:office 裁剪臂消费 mountPreview,此处观察调用形。
const mountPreviewMock = vi.fn();
vi.mock("./router", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./router")>();
  return {
    ...actual,
    mountPreview: (...args: unknown[]) => mountPreviewMock(...args),
  };
});

import { mountCover } from "./cover";
import { blob, okResponse, settle, stubFetch, THUMB_URL, unstubAll } from "./testutil";
import type { PreviewRequest } from "./types";

beforeEach(() => {
  document.body.innerHTML = "";
  mountPreviewMock.mockReset().mockReturnValue({ destroy: vi.fn() });
});

afterEach(() => {
  unstubAll();
});

function req(kind: PreviewRequest["kind"], over: Partial<PreviewRequest> = {}): PreviewRequest {
  return {
    kind,
    urls: { thumb: THUMB_URL, raw: "http://127.0.0.1:1/f/h/raw" },
    meta: { name: `样本.${kind}`, size: 1024 },
    ...over,
  };
}

function mount(r: PreviewRequest): { host: HTMLElement; handle: ReturnType<typeof mountCover> } {
  const host = document.createElement("div");
  document.body.appendChild(host);
  return { host, handle: mountCover(host, r) };
}

describe("cover:img 面(image=thumb;video=海报帧;失败=图标)", () => {
  it("image → <img> 挂 thumb URL", () => {
    stubFetch({});
    const { host, handle } = mount(req("image"));
    const img = host.querySelector<HTMLImageElement>(".mtdss-cover__img");
    expect(img).not.toBeNull();
    expect(img!.src).toBe(THUMB_URL);
    handle.destroy();
    expect(host.querySelector(".mtdss-cover__img")).toBeNull(); // destroy 摘净
  });

  it("video → <img> 挂海报帧 thumb 面;加载失败 → 视频图标", () => {
    stubFetch({});
    const { host, handle } = mount(req("video"));
    const img = host.querySelector<HTMLImageElement>(".mtdss-cover__img");
    expect(img).not.toBeNull();
    expect(img!.src).toBe(THUMB_URL);
    img!.dispatchEvent(new Event("error"));
    expect(host.querySelector(".mtdss-cover__icon svg")).not.toBeNull();
    handle.destroy();
  });

  it("image thumb 404(heic 无帧等同)→ 文件图标", () => {
    stubFetch({});
    const { host, handle } = mount(req("image", { urls: {} }));
    const img = host.querySelector<HTMLImageElement>(".mtdss-cover__img");
    expect(img).not.toBeNull();
    img!.dispatchEvent(new Event("error"));
    expect(host.querySelector(".mtdss-cover__icon")).not.toBeNull();
    handle.destroy();
  });
});

describe("cover:图标面(audio/unsupported)", () => {
  it("audio → 音符图标;unsupported → 文件图标(零请求零文本)", () => {
    stubFetch({});
    const a = mount(req("audio"));
    expect(a.host.querySelector(".mtdss-cover__icon svg")).not.toBeNull();
    a.handle.destroy();
    const u = mount(req("unsupported"));
    expect(u.host.querySelector(".mtdss-cover__icon svg")).not.toBeNull();
    expect(u.host.textContent).not.toContain("http");
    u.handle.destroy();
  });
});

describe("cover:文本前段(text/md)", () => {
  it("Range 取头窗 → <pre> 前 24 行;请求失败 → 文件图标", async () => {
    const text = Array.from({ length: 40 }, (_, i) => `第${i + 1}行`).join("\n");
    stubFetch({ "http://127.0.0.1:1/f/h/raw": okResponse(blob(text.length, "text/plain")) });
    // 桩返回的是空壳字节——改用真实文本Response覆盖
    globalThis.fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      expect(String(input)).toBe("http://127.0.0.1:1/f/h/raw");
      expect((init?.headers as Record<string, string>)["Range"]).toBe("bytes=0-4095");
      return new Response(text, { status: 200 });
    }) as typeof fetch;
    const { host, handle } = mount(req("text"));
    await settle();
    const pre = host.querySelector<HTMLPreElement>(".mtdss-cover__pre");
    expect(pre).not.toBeNull();
    expect(pre!.textContent).toContain("第1行");
    expect(pre!.textContent).toContain("第24行");
    expect(pre!.textContent).not.toContain("第25行");
    handle.destroy();

    globalThis.fetch = vi.fn(async () => new Response("", { status: 500 })) as typeof fetch;
    const f = mount(req("md"));
    await settle();
    expect(f.host.querySelector(".mtdss-cover__icon")).not.toBeNull();
    f.handle.destroy();
  });
});

describe("cover:pdf 首页 canvas(pdf.js 假工)", () => {
  it("getDocument(url) → page 1 → canvas 落地(失败兜底=文件图标,同 text 失败例的 catch 面)", async () => {
    stubFetch({});
    const { host, handle } = mount(req("pdf"));
    await settle();
    await settle();
    const canvas = host.querySelector<HTMLCanvasElement>(".mtdss-cover__canvas");
    expect(canvas).not.toBeNull();
    handle.destroy();
  });
});

describe("cover:office 裁剪呈现(尺寸门)", () => {
  it("≤门 → mountPreview 进裁剪壳(destroy 透传);>门 → 文件图标", async () => {
    stubFetch({});
    const ok = mount(req("docx", { meta: { name: "小.docx", size: 1024 } }));
    await settle();
    expect(mountPreviewMock).toHaveBeenCalledTimes(1);
    expect(mountPreviewMock.mock.calls[0][0].className).toContain("mtdss-cover__clip");
    const inner = { destroy: vi.fn() };
    mountPreviewMock.mockReturnValue(inner);
    const h2 = mount(req("pptx", { meta: { name: "小.pptx", size: 2048 } }));
    await settle();
    h2.handle.destroy();
    expect(inner.destroy).toHaveBeenCalledTimes(1);
    ok.handle.destroy();

    const big = mount(
      req("xlsx", { meta: { name: "巨.xlsx", size: 26 * 1024 * 1024 } }),
    );
    await settle();
    expect(big.host.querySelector(".mtdss-cover__icon")).not.toBeNull();
    expect(mountPreviewMock).toHaveBeenCalledTimes(2); // 未增(超门不挂)
    big.handle.destroy();
  });
});

describe("cover:文本 BOM 嗅探(field r14 #2,ini「16 进制」=UTF-16 误读)", () => {
  it("UTF-16LE BOM → 按 utf-16le 解码可读;UTF-8 BOM 剥离", async () => {
    const utf16 = new TextEncoder().encode("﻿").slice(0, 0); // placeholder no-op
    void utf16;
    const make16 = (s: string): ArrayBuffer => {
      const out = new Uint8Array(2 + s.length * 2);
      out[0] = 0xff;
      out[1] = 0xfe;
      for (let i = 0; i < s.length; i += 1) {
        const cp = s.charCodeAt(i);
        out[2 + i * 2] = cp & 0xff;
        out[3 + i * 2] = cp >> 8;
      }
      return out.buffer;
    };
    globalThis.fetch = vi.fn(async () => new Response(make16("[配置]\r\n键=值"), { status: 200 })) as typeof fetch;
    const a = mount(req("text"));
    await settle();
    const pre = a.host.querySelector<HTMLPreElement>(".mtdss-cover__pre");
    expect(pre).not.toBeNull();
    expect(pre!.textContent).toContain("[配置]");
    expect(pre!.textContent).toContain("键=值");
    a.handle.destroy();

    const u8 = new Uint8Array([0xef, 0xbb, 0xbf, ...new TextEncoder().encode("纯utf8行")]);
    globalThis.fetch = vi.fn(async () => new Response(u8.buffer, { status: 200 })) as typeof fetch;
    const b = mount(req("md"));
    await settle();
    const pre2 = b.host.querySelector<HTMLPreElement>(".mtdss-cover__pre");
    expect(pre2!.textContent).toContain("纯utf8行");
    expect(pre2!.textContent).not.toContain("﻿");
    b.handle.destroy();
  });
});
