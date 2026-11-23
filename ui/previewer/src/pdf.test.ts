// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { fakePdfjsState, fakeRangesFor, resetFakePdfjs } from "./pdfjs-fake";

vi.mock("pdfjs-dist", async () => {
  const fake = await import("./pdfjs-fake");
  return fake.makeFakePdfjs();
});
vi.mock("pdfjs-dist/build/pdf.worker.min.mjs", () => ({}));

import { mountPreview } from "./router";
import { PDF_BATCH, PDF_INITIAL_BYTES } from "./pdf";
import { defaultPreviewStrings } from "./strings";
import {
  bytesResponse,
  deferred,
  eventLog,
  rangeHeaderOf,
  RAW_URL,
  sequentialFetch,
  settle,
  stubFetch,
  unstubAll,
} from "./testutil";
import type { PreviewEvent, PreviewRequest } from "./types";

const TOTAL = PDF_INITIAL_BYTES + 1000; // 首取窗盖不满全文件 → 必有续取面

function initialChunk(): Uint8Array {
  const data = new Uint8Array(PDF_INITIAL_BYTES);
  for (let i = 0; i < data.length; i += 1) data[i] = i % 256;
  return data;
}

function tailChunk(len = 64): Uint8Array {
  const data = new Uint8Array(len);
  for (let i = 0; i < len; i += 1) data[i] = (i + 7) % 256;
  return data;
}

function pdfReq(name = "样本.pdf"): PreviewRequest {
  return { kind: "pdf", urls: { raw: RAW_URL }, meta: { name, size: TOTAL } };
}

function mount(req: PreviewRequest, events = eventLog<PreviewEvent>()) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const handle = mountPreview(el, req, events.onEvent);
  const scroller = () => el.querySelector<HTMLElement>(".mtdss-preview__pdf-scroll");
  const canvases = () => el.querySelectorAll("canvas.mtdss-preview__pdf-canvas");
  const indicator = () => el.querySelector<HTMLElement>(".mtdss-preview__indicator");
  const moreButton = () => el.querySelector<HTMLButtonElement>("button.mtdss-preview__pdf-more");
  const overlay = () => el.querySelector<HTMLElement>(".mtdss-preview__overlay");
  const retryButton = () => el.querySelector<HTMLButtonElement>(".mtdss-preview__overlay button");
  return { el, handle, events: events.events, scroller, canvases, indicator, moreButton, overlay, retryButton };
}

function errRangeResponse(): Response {
  return {
    ok: false,
    status: 503,
    headers: { get: () => null },
    arrayBuffer: () => Promise.reject(new Error("no body")),
  } as unknown as Response;
}

beforeEach(() => {
  document.body.innerHTML = "";
});

afterEach(() => {
  unstubAll();
});

describe("pdf 首取:range 序列与三页装配", () => {
  it("首取 Range bytes=0-{PDF_INITIAL_BYTES-1};transport 记 length/initialData;先渲 3 页", async () => {
    resetFakePdfjs(9);
    const head = initialChunk();
    const fetchMock = sequentialFetch([bytesResponse(head, { begin: 0, total: TOTAL })]);

    const view = mount(pdfReq());
    await settle();

    // 初始 range 请求:序列恰一枚,窗=常量(≈3 页字节量级,对齐 daemon 预读窗)
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(rangeHeaderOf(fetchMock.mock.calls[0] as unknown[])).toBe(`bytes=0-${PDF_INITIAL_BYTES - 1}`);

    // transport 接线:length=总长、initialData=首取窗字节、getDocument 恰收该 transport
    const st = fakePdfjsState();
    expect(st.transport).not.toBeNull();
    expect(st.transport!.length).toBe(TOTAL);
    expect(st.transport!.initialData!.length).toBe(PDF_INITIAL_BYTES);
    expect(st.transport!.initialData![0]).toBe(0);
    expect(st.transport!.initialData![PDF_INITIAL_BYTES - 1]).toBe((PDF_INITIAL_BYTES - 1) % 256);
    expect(st.docParams).toHaveLength(1);
    expect(st.docParams[0].range).toBe(st.transport);

    // 先渲 3 页:画布 3 枚、页码指示「第 3 / 9 页」、meta 逐页携页码
    expect(view.canvases().length).toBe(PDF_BATCH);
    expect(view.indicator()?.textContent).toBe("第 3 / 9 页");
    expect(view.events).toEqual([
      { type: "meta", bytes: TOTAL, page: 1, pages: 9 },
      { type: "meta", bytes: TOTAL, page: 2, pages: 9 },
      { type: "meta", bytes: TOTAL, page: 3, pages: 9 },
    ]);
    // 尚有未渲页:续载钮在
    expect(view.moreButton()?.hidden).toBe(false);
    expect(view.moreButton()?.textContent).toBe(defaultPreviewStrings.pdfLoadMore);
    view.handle.destroy();
  });

  it("小 PDF(首窗盖全):单请求即渲至页尽,续载钮不出现", async () => {
    resetFakePdfjs(3);
    const head = initialChunk();
    const fetchMock = sequentialFetch([bytesResponse(head, { begin: 0, total: PDF_INITIAL_BYTES })]);

    const view = mount(pdfReq());
    await settle();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(view.canvases().length).toBe(3);
    expect(view.indicator()?.textContent).toBe("第 3 / 3 页");
    expect(view.moreButton()?.hidden).toBe(true);
    expect(view.events).toEqual([
      { type: "meta", bytes: PDF_INITIAL_BYTES, page: 1, pages: 3 },
      { type: "meta", bytes: PDF_INITIAL_BYTES, page: 2, pages: 3 },
      { type: "meta", bytes: PDF_INITIAL_BYTES, page: 3, pages: 3 },
    ]);
    view.handle.destroy();
  });
});

describe("pdf 续取:「查看更多」与滚动近底", () => {
  it("点「查看更多」:下一页装配,pdf.js 经 transport 请求后续 range(请求序列断言)", async () => {
    resetFakePdfjs(9);
    fakeRangesFor(4, [[PDF_INITIAL_BYTES, PDF_INITIAL_BYTES + 65535]]);
    const fetchMock = sequentialFetch([
      bytesResponse(initialChunk(), { begin: 0, total: TOTAL }),
      bytesResponse(tailChunk(65536), { begin: PDF_INITIAL_BYTES, total: TOTAL }),
    ]);

    const view = mount(pdfReq());
    await settle();
    expect(view.canvases().length).toBe(3);

    view.moreButton()!.click();
    await settle();

    // 续取序列:第 2 请求=pdf.js 要的精确 range;第 4 页画布与页码到位
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(rangeHeaderOf(fetchMock.mock.calls[1] as unknown[])).toBe(
      `bytes=${PDF_INITIAL_BYTES}-${PDF_INITIAL_BYTES + 65535}`,
    );
    expect(view.canvases().length).toBe(4);
    expect(view.indicator()?.textContent).toBe("第 4 / 9 页");
    expect(view.events[view.events.length - 1]).toEqual({ type: "meta", bytes: TOTAL, page: 4, pages: 9 });
    view.handle.destroy();
  });

  it("滚动近底:同款续取(阈值内触发);远离底部不触发", async () => {
    resetFakePdfjs(9);
    fakeRangesFor(4, [[PDF_INITIAL_BYTES, PDF_INITIAL_BYTES + 65535]]);
    const fetchMock = sequentialFetch([
      bytesResponse(initialChunk(), { begin: 0, total: TOTAL }),
      bytesResponse(tailChunk(65536), { begin: PDF_INITIAL_BYTES, total: TOTAL }),
    ]);

    const view = mount(pdfReq());
    await settle();
    const scrollerEl = view.scroller()!;
    Object.defineProperty(scrollerEl, "scrollHeight", { value: 10000, configurable: true });
    Object.defineProperty(scrollerEl, "clientHeight", { value: 800, configurable: true });

    // 远离底部:不触发
    scrollerEl.scrollTop = 2000;
    scrollerEl.dispatchEvent(new Event("scroll"));
    await settle();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(view.canvases().length).toBe(3);

    // 近底(距底 < 240px):触发续取
    scrollerEl.scrollTop = 9500; // 9500+800 >= 10000-240
    scrollerEl.dispatchEvent(new Event("scroll"));
    await settle();
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(rangeHeaderOf(fetchMock.mock.calls[1] as unknown[])).toBe(
      `bytes=${PDF_INITIAL_BYTES}-${PDF_INITIAL_BYTES + 65535}`,
    );
    expect(view.canvases().length).toBe(4);
    view.handle.destroy();
  });

  it("页尽后:续载钮隐没,滚动零新请求", async () => {
    resetFakePdfjs(3);
    const fetchMock = sequentialFetch([bytesResponse(initialChunk(), { begin: 0, total: PDF_INITIAL_BYTES })]);
    const view = mount(pdfReq());
    await settle();
    expect(view.moreButton()?.hidden).toBe(true);

    const scrollerEl = view.scroller()!;
    Object.defineProperty(scrollerEl, "scrollHeight", { value: 10000, configurable: true });
    Object.defineProperty(scrollerEl, "clientHeight", { value: 800, configurable: true });
    scrollerEl.scrollTop = 9999;
    scrollerEl.dispatchEvent(new Event("scroll"));
    await settle();
    expect(fetchMock).toHaveBeenCalledTimes(1); // 页尽:滚动零新请求
    view.handle.destroy();
  });
});

describe("pdf 失败面:retry_needed 与重试", () => {
  it("首取失败:覆盖层+重试钮+retry_needed 恰一枚;重试恢复 → 三页到位", async () => {
    resetFakePdfjs(5);
    const fetchMock = sequentialFetch([errRangeResponse(), bytesResponse(initialChunk(), { begin: 0, total: TOTAL })]);

    const view = mount(pdfReq());
    await settle();
    expect(view.overlay()!.classList.contains("mtdss-preview__overlay--hidden")).toBe(false);
    expect(view.retryButton()?.textContent).toBe(defaultPreviewStrings.retry);
    expect(view.events).toEqual([{ type: "retry_needed", message: defaultPreviewStrings.pdfLoadFailed }]);
    expect(view.canvases().length).toBe(0);

    view.retryButton()!.click();
    await settle();
    expect(view.overlay()!.classList.contains("mtdss-preview__overlay--hidden")).toBe(true);
    expect(view.canvases().length).toBe(PDF_BATCH);
    expect(view.events[view.events.length - 1]).toEqual({ type: "meta", bytes: TOTAL, page: 3, pages: 5 });
    expect(fetchMock).toHaveBeenCalledTimes(2); // 首取+重试各一枚
    view.handle.destroy();
  });

  it("raw 缺席:终态 error,零 fetch", async () => {
    resetFakePdfjs(5);
    const fetchMock = stubFetch({});
    const view = mount({ kind: "pdf", urls: {}, meta: { name: "样本.pdf", size: 1 } });
    await settle();
    expect(view.events).toEqual([{ type: "error", message: defaultPreviewStrings.unavailableMessage }]);
    expect(fetchMock).not.toHaveBeenCalled();
    view.handle.destroy();
  });

  it("续取 range 失败:覆盖层出+retry_needed;重试断点续渲", async () => {
    resetFakePdfjs(9);
    fakeRangesFor(4, [[PDF_INITIAL_BYTES, PDF_INITIAL_BYTES + 65535]]);
    const queue = [
      bytesResponse(initialChunk(), { begin: 0, total: TOTAL }),
      errRangeResponse(),
      bytesResponse(tailChunk(65536), { begin: PDF_INITIAL_BYTES, total: TOTAL }),
    ];
    const fetchMock = vi.fn(() => {
      const res = queue.shift();
      if (res === undefined) return Promise.reject(new Error("unexpected extra fetch"));
      return Promise.resolve(res);
    });
    vi.stubGlobal("fetch", fetchMock);

    const view = mount(pdfReq());
    await settle();
    expect(view.canvases().length).toBe(3);

    view.moreButton()!.click();
    await settle();
    // transport range fetch 失败 → 覆盖层 + retry_needed
    expect(view.overlay()!.classList.contains("mtdss-preview__overlay--hidden")).toBe(false);
    expect(view.events[view.events.length - 1]).toEqual({ type: "retry_needed", message: defaultPreviewStrings.pdfLoadFailed });

    view.retryButton()!.click();
    await settle();
    expect(view.overlay()!.classList.contains("mtdss-preview__overlay--hidden")).toBe(true);
    expect(view.canvases().length).toBe(4);
    expect(view.events[view.events.length - 1]).toEqual({ type: "meta", bytes: TOTAL, page: 4, pages: 9 });
    expect(fetchMock).toHaveBeenCalledTimes(3);
    view.handle.destroy();
  });
});

describe("pdf 文案缝与 destroy", () => {
  it("文案覆写:续载钮与页码模板按 strings 替换", async () => {
    resetFakePdfjs(9);
    sequentialFetch([bytesResponse(initialChunk(), { begin: 0, total: TOTAL })]);
    const view = mount({
      ...pdfReq(),
      strings: { pdfLoadMore: "More", pdfPageIndicator: "p {page}/{total}" },
    });
    await settle();
    expect(view.moreButton()?.textContent).toBe("More");
    expect(view.indicator()?.textContent).toBe("p 3/9");
    view.handle.destroy();
  });

  it("首取在途中 destroy:请求 abort、DOM 摘净、迟火零事件", async () => {
    resetFakePdfjs(9);
    const pending = deferred<Response>();
    const fetchMock = vi.fn((_url: unknown, _init?: RequestInit) => pending.promise);
    vi.stubGlobal("fetch", fetchMock);

    const view = mount(pdfReq());
    await settle();
    expect(view.canvases().length).toBe(0);

    view.handle.destroy();
    expect(fetchMock.mock.calls[0][1]?.signal?.aborted).toBe(true);
    expect(view.el.querySelector(".mtdss-preview")).toBeNull();

    pending.resolve(bytesResponse(initialChunk(), { begin: 0, total: TOTAL }));
    await settle();
    expect(view.canvases().length).toBe(0);
    expect(view.events).toEqual([]); // 迟火零事件外泄
  });
});
