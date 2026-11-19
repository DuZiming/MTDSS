// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("docx-preview", async () => {
  const fake = await import("./office-fake");
  return fake.makeFakeDocxPreview();
});
vi.mock("xlsx", async () => {
  const fake = await import("./office-fake");
  return fake.makeFakeXlsx();
});
vi.mock("@file-viewer/pptx", async () => {
  const fake = await import("./office-fake");
  return fake.makeFakePptx();
});
vi.mock("./pptx-worker", () => ({
  createPptxParserWorker: () =>
    ({ postMessage() {}, addEventListener() {}, terminate() {} }) as unknown as Worker,
}));

import { mountPreview } from "./router";
import { defaultPreviewStrings } from "./strings";
import {
  deferred,
  errResponse,
  eventLog,
  fullBytesResponse,
  rangeHeaderOf,
  RAW_URL,
  sequentialFetch,
  settle,
  settleUntil,
  stubFetch,
  unstubAll,
} from "./testutil";
import { OFFICE_BATCH, XLSX_WINDOW_ROWS } from "./office";
import {
  fakeDocxState,
  fakePptxState,
  fakeXlsxState,
  resetFakeDocx,
  resetFakePptx,
  resetFakeXlsx,
} from "./office-fake";
import type { PreviewEvent, PreviewRequest } from "./types";

const BYTES = new Uint8Array(64).fill(0x61);

function officeReq(
  kind: "docx" | "xlsx" | "pptx",
  urls: PreviewRequest["urls"] = { raw: RAW_URL },
  name = `样本.${kind}`,
  strings?: PreviewRequest["strings"],
): PreviewRequest {
  return {
    kind,
    urls,
    meta: { name, size: BYTES.length },
    ...(strings === undefined ? {} : { strings }),
  };
}

function mount(req: PreviewRequest, events = eventLog<PreviewEvent>()) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const handle = mountPreview(el, req, events.onEvent);
  return {
    el,
    handle,
    events: events.events,
    root: () => el.querySelector<HTMLElement>(".mtdss-preview--office"),
    loading: () => el.querySelector<HTMLElement>(".mtdss-preview__office-loading"),
    loadingNote: () =>
      el.querySelector<HTMLElement>(".mtdss-preview__office-loading .mtdss-preview__note"),
    pageSlots: () => el.querySelectorAll<HTMLElement>(".mtdss-preview__office-page"),
    moreButton: () => el.querySelector<HTMLButtonElement>("button.mtdss-preview__office-more"),
    sheetScroller: () => el.querySelector<HTMLElement>(".mtdss-preview__sheet-scroll"),
    tabs: () => el.querySelectorAll<HTMLButtonElement>("button.mtdss-preview__sheet-tab"),
    tbodyRows: () =>
      el.querySelectorAll<HTMLTableRowElement>(".mtdss-preview__sheet-table tbody tr"),
    xlsxMore: () => el.querySelector<HTMLButtonElement>("button.mtdss-preview__xlsx-more"),
    pptxTarget: () => el.querySelector<HTMLElement>(".mtdss-preview__office-pptx"),
    placeholderName: () => el.querySelector<HTMLElement>(".mtdss-preview__placeholder-name"),
    placeholderNote: () => el.querySelector<HTMLElement>(".mtdss-preview__placeholder-note"),
    downloadLink: () => el.querySelector<HTMLAnchorElement>("a.mtdss-preview__download-link"),
  };
}

/// 二维表构造:首行表头 + (totalRows-1) 行体(字符串与数字混型,钉 String 化值面)。
function sheetRows(totalRows: number): unknown[][] {
  const rows: unknown[][] = [["列A", "列B"]];
  for (let i = 1; i < totalRows; i += 1) rows.push([`r${i}`, i]);
  return rows;
}

function assertUnsupported(
  view: ReturnType<typeof mount>,
  name: string,
  withDownload = true,
): void {
  expect(view.root()).toBeNull(); // office DOM 已摘
  expect(view.placeholderName()?.textContent).toBe(name);
  expect(view.placeholderNote()?.textContent).toBe(defaultPreviewStrings.unsupportedNote);
  if (withDownload) {
    expect(view.downloadLink()?.getAttribute("href")).toBe(RAW_URL);
    expect(view.downloadLink()?.getAttribute("download")).toBe(name);
  } else {
    expect(view.downloadLink()).toBeNull();
  }
  expect(view.events).toEqual([{ type: "error", message: defaultPreviewStrings.officeLoadFailed }]);
}

beforeEach(() => {
  document.body.innerHTML = "";
  document.getElementById("mtdss-preview-styles")?.remove();
});

afterEach(() => {
  unstubAll();
});

describe("docx:整取与渐进装配", () => {
  it("整取=单枚全量请求(无 Range 头)+挂载即 spinner 文案+先渲 3 页+meta 就绪", async () => {
    resetFakeDocx({ pages: 5 });
    const fetchMock = sequentialFetch([fullBytesResponse(BYTES)]);

    const view = mount(officeReq("docx"));
    // 挂载即出加载面(spinner+文案),零页内容
    expect(view.loading()).not.toBeNull();
    expect(view.loadingNote()?.textContent).toBe(defaultPreviewStrings.officeLoading);
    expect(view.pageSlots().length).toBe(0);

    await settleUntil(() => view.pageSlots().length > 0);
    await settle();

    // 整取:恰一枚、无 Range 头、字节原样达解析器(inWrapper:false 拆页 + renderAltChunks:false 摘原始 HTML 通路)
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(rangeHeaderOf(fetchMock.mock.calls[0] as unknown[])).toBeNull();
    expect(fakeDocxState().parseCalls[0]?.byteLength).toBe(BYTES.length);
    expect(fakeDocxState().parseCalls[0]?.opts).toMatchObject({
      inWrapper: false,
      renderAltChunks: false,
    });

    // 先渲 3 页、加载面撤、meta 主内容就绪、「查看更多」在
    expect(view.pageSlots().length).toBe(OFFICE_BATCH);
    expect(view.loading()).toBeNull();
    expect(view.moreButton()?.textContent).toBe(defaultPreviewStrings.officeLoadMore);
    expect(view.events).toEqual([{ type: "meta", bytes: BYTES.length, page: 3, pages: 5 }]);
    view.handle.destroy();
  });

  it("「查看更多」逐段续渲:3→6→7 页尽钮拆,meta 逐段更新", async () => {
    resetFakeDocx({ pages: 7 });
    sequentialFetch([fullBytesResponse(BYTES)]);

    const view = mount(officeReq("docx"));
    await settleUntil(() => view.pageSlots().length === OFFICE_BATCH);

    view.moreButton()!.click();
    await settle();
    expect(view.pageSlots().length).toBe(OFFICE_BATCH * 2);
    expect(view.moreButton()).not.toBeNull();

    view.moreButton()!.click();
    await settle();
    expect(view.pageSlots().length).toBe(7);
    expect(view.moreButton()?.hidden).toBe(true); // 页尽:钮/栏隐没(不留死空条)
    expect(view.events).toEqual([
      { type: "meta", bytes: BYTES.length, page: 3, pages: 7 },
      { type: "meta", bytes: BYTES.length, page: 6, pages: 7 },
      { type: "meta", bytes: BYTES.length, page: 7, pages: 7 },
    ]);
    view.handle.destroy();
  });

  it("渲染失败(renderDocument 抛)→ unsupported 形+error 事件", async () => {
    resetFakeDocx({ pages: 3, failRender: true });
    sequentialFetch([fullBytesResponse(BYTES)]);

    const view = mount(officeReq("docx"));
    await settle();
    assertUnsupported(view, "样本.docx");
    view.handle.destroy();
  });

  it("解析失败(parseAsync 抛)→ unsupported 形", async () => {
    resetFakeDocx({ pages: 3, failParse: true });
    sequentialFetch([fullBytesResponse(BYTES)]);

    const view = mount(officeReq("docx"));
    await settle();
    assertUnsupported(view, "样本.docx");
    view.handle.destroy();
  });

  it("整取失败(!ok)→ unsupported 形", async () => {
    resetFakeDocx({ pages: 3 });
    sequentialFetch([errResponse(503)]);

    const view = mount(officeReq("docx"));
    await settle();
    assertUnsupported(view, "样本.docx");
    view.handle.destroy();
  });

  it("零可渲染页(纯样式节点)→ unsupported 形", async () => {
    resetFakeDocx({ pages: 0 });
    sequentialFetch([fullBytesResponse(BYTES)]);

    const view = mount(officeReq("docx"));
    await settle();
    assertUnsupported(view, "样本.docx");
    view.handle.destroy();
  });

  it("raw 缺 → 即降级 unsupported(零 fetch,静态下载引导)", async () => {
    resetFakeDocx({ pages: 3 });
    const fetchMock = stubFetch({});

    const view = mount(officeReq("docx", {}));
    await settle();
    expect(fetchMock).not.toHaveBeenCalled();
    assertUnsupported(view, "样本.docx", false);
    view.handle.destroy();
  });
});

describe("xlsx:行窗与 sheet 标签", () => {
  it("首屏恰 500 行(表头+499 体行)+值字符串化+meta 行窗面", async () => {
    resetFakeXlsx({ sheets: [{ name: "表A", rows: sheetRows(1401) }] });
    const fetchMock = sequentialFetch([fullBytesResponse(BYTES)]);

    const view = mount(officeReq("xlsx"));
    await settleUntil(() => view.tbodyRows().length > 0);
    await settle();

    expect(fetchMock).toHaveBeenCalledTimes(1); // 整取单枚
    expect(view.tbodyRows().length).toBe(XLSX_WINDOW_ROWS - 1);
    expect(view.tabs().length).toBe(1);
    expect(view.tabs()[0]?.textContent).toBe("表A");
    expect(view.tabs()[0]?.className).toContain("mtdss-preview__sheet-tab--active");
    expect(view.el.querySelector(".mtdss-preview__sheet-table thead")?.textContent).toContain("列A");
    const firstBody = view.tbodyRows()[0]?.textContent ?? "";
    expect(firstBody).toContain("r1");
    expect(firstBody).toContain("1"); // 数字单元格 String 化(零类型逃逸)
    // 字符串化读面:{header:1, raw:false, defval:""} 钉形
    expect(fakeXlsxState().toJsonOpts[0]).toMatchObject({ header: 1, raw: false, defval: "" });
    expect(view.xlsxMore()?.textContent).toBe(defaultPreviewStrings.xlsxLoadMore);
    expect(view.events).toEqual([
      { type: "meta", bytes: BYTES.length, page: XLSX_WINDOW_ROWS, pages: 1401 },
    ]);
    view.handle.destroy();
  });

  it("「加载更多」翻页:恰 +500 行窗,续渲零请求,页尽钮拆", async () => {
    resetFakeXlsx({ sheets: [{ name: "表A", rows: sheetRows(1401) }] });
    const fetchMock = sequentialFetch([fullBytesResponse(BYTES)]);

    const view = mount(officeReq("xlsx"));
    await settleUntil(() => view.tbodyRows().length > 0);

    view.xlsxMore()!.click();
    await settle();
    expect(view.tbodyRows().length).toBe(XLSX_WINDOW_ROWS * 2 - 1);
    // 体行 k = sheet 行 k:999 体行末枚 = 行 999(文本面 = "r999"+"999" 拼接)
    expect(view.tbodyRows()[view.tbodyRows().length - 1]?.textContent).toContain("r999");

    view.xlsxMore()!.click(); // 末窗:1000 → 1401 尽
    await settle();
    expect(view.tbodyRows().length).toBe(1400);
    expect(view.xlsxMore()?.hidden).toBe(true); // 页尽:钮/栏隐没
    view.xlsxMore()!.click(); // 页尽后再点:守卫面零新行
    expect(view.tbodyRows().length).toBe(1400);
    expect(fetchMock).toHaveBeenCalledTimes(1); // 续渲零请求(整取一枚)
    expect(view.events).toEqual([
      { type: "meta", bytes: BYTES.length, page: 500, pages: 1401 },
      { type: "meta", bytes: BYTES.length, page: 1000, pages: 1401 },
      { type: "meta", bytes: BYTES.length, page: 1401, pages: 1401 },
    ]);
    view.handle.destroy();
  });

  it("sheet 标签切换:窗重置回 500+标签态互斥+meta 更新", async () => {
    resetFakeXlsx({
      sheets: [
        { name: "表A", rows: sheetRows(1401) },
        { name: "表B", rows: [["X"], ["x1"], ["x2"], ["x3"], ["x4"]] }, // 5 行<500
      ],
    });
    sequentialFetch([fullBytesResponse(BYTES)]);

    const view = mount(officeReq("xlsx"));
    await settleUntil(() => view.tbodyRows().length > 0);
    expect(view.tabs().length).toBe(2);

    view.tabs()[1]!.click();
    await settle();
    expect(view.tabs()[1]?.className).toContain("mtdss-preview__sheet-tab--active");
    expect(view.tabs()[0]?.className).not.toContain("mtdss-preview__sheet-tab--active");
    expect(view.tbodyRows().length).toBe(4);
    expect(view.xlsxMore()?.hidden).toBe(true); // 5 行 < 窗:钮/栏隐没
    expect(view.events[view.events.length - 1]).toEqual({ type: "meta", bytes: BYTES.length, page: 5, pages: 5 });

    view.tabs()[0]!.click(); // 切回 A:窗重置
    await settle();
    expect(view.tbodyRows().length).toBe(XLSX_WINDOW_ROWS - 1);
    expect(view.events[view.events.length - 1]).toEqual({
      type: "meta",
      bytes: BYTES.length,
      page: XLSX_WINDOW_ROWS,
      pages: 1401,
    });
    view.handle.destroy();
  });

  it("解析失败(read 抛)→ unsupported 形", async () => {
    resetFakeXlsx({ sheets: [{ name: "表A", rows: sheetRows(10) }], failParse: true });
    sequentialFetch([fullBytesResponse(BYTES)]);

    const view = mount(officeReq("xlsx"));
    await settle();
    assertUnsupported(view, "样本.xlsx");
    view.handle.destroy();
  });

  it("滚动近底触发续行(钮/滚双入口同窗),远离底部不触发", async () => {
    resetFakeXlsx({ sheets: [{ name: "表A", rows: sheetRows(1401) }] });
    sequentialFetch([fullBytesResponse(BYTES)]);

    const view = mount(officeReq("xlsx"));
    await settleUntil(() => view.tbodyRows().length > 0);
    const sc = view.sheetScroller()!;
    Object.defineProperty(sc, "scrollHeight", { value: 10000, configurable: true });
    Object.defineProperty(sc, "clientHeight", { value: 800, configurable: true });

    sc.scrollTop = 2000;
    sc.dispatchEvent(new Event("scroll"));
    await settle();
    expect(view.tbodyRows().length).toBe(XLSX_WINDOW_ROWS - 1); // 远离底部:不触发

    sc.scrollTop = 9500; // 9500+800 >= 10000-240
    sc.dispatchEvent(new Event("scroll"));
    await settle();
    expect(view.tbodyRows().length).toBe(XLSX_WINDOW_ROWS * 2 - 1);
    view.handle.destroy();
  });
});

describe("pptx:viewer 接线与降级", () => {
  it("viewer 接线钉面:windowed+initialSlides=3+buffer 全量+meta 两面(首页就绪/渲染完)", async () => {
    resetFakePptx({ slides: 8 });
    const fetchMock = sequentialFetch([fullBytesResponse(BYTES)]);

    const view = mount(officeReq("pptx"));
    const op = () => fakePptxState().opens[0];
    await settleUntil(() => op() !== undefined);
    expect(fetchMock).toHaveBeenCalledTimes(1); // 整取单枚
    expect(op()!.byteLength).toBe(BYTES.length);
    expect(op()!.targetClass).toBe("mtdss-preview__office-pptx");
    expect(op()!.options.listOptions).toEqual({
      windowed: true,
      initialSlides: OFFICE_BATCH,
      batchSize: OFFICE_BATCH,
    });
    expect(typeof op()!.options.workerFactory).toBe("function");

    await settleUntil(() => view.pptxTarget()?.children.length === 8);
    expect(view.loading()).toBeNull();
    expect(view.events).toEqual([
      { type: "meta", bytes: BYTES.length, page: 1, pages: 8 },
      { type: "meta", bytes: BYTES.length, page: 8, pages: 8 },
    ]);
    view.handle.destroy();
    expect(fakePptxState().destroyed).toBe(1); // destroy 收口 viewer
  });

  it("open 失败(worker 起不来)→ unsupported 形", async () => {
    resetFakePptx({ slides: 4, failOpen: true });
    sequentialFetch([fullBytesResponse(BYTES)]);

    const view = mount(officeReq("pptx"));
    await settle();
    assertUnsupported(view, "样本.pptx");
    view.handle.destroy();
  });

  it("渲染中途 onError → unsupported 形", async () => {
    resetFakePptx({ slides: 4, failRender: true });
    sequentialFetch([fullBytesResponse(BYTES)]);

    const view = mount(officeReq("pptx"));
    await settleUntil(() => view.placeholderName() !== null);
    assertUnsupported(view, "样本.pptx");
    view.handle.destroy();
    expect(fakePptxState().destroyed).toBe(1); // 降级后 destroy 仍收口 viewer
  });

  it("零幻灯片(渲染完零内容)→ unsupported 形", async () => {
    resetFakePptx({ slides: 0 });
    sequentialFetch([fullBytesResponse(BYTES)]);

    const view = mount(officeReq("pptx"));
    // open 达 → 下一拍消息流(零 slide → onRenderComplete 即降级):双拍宏任务
    await settleUntil(() => view.placeholderName() !== null);
    assertUnsupported(view, "样本.pptx");
    view.handle.destroy();
  });

  it("open 在途 destroy:迟到的 viewer 即收口、零事件、降级不发生", async () => {
    resetFakePptx({ slides: 4, deferOpen: true });
    sequentialFetch([fullBytesResponse(BYTES)]);

    const view = mount(officeReq("pptx"));
    await settleUntil(() => fakePptxState().openGate !== null);
    view.handle.destroy();
    expect(view.el.querySelector(".mtdss-preview")).toBeNull();

    fakePptxState().openGate!.resolve({
      slideCount: 4,
      destroy: () => {
        fakePptxState().destroyed += 1;
      },
    });
    await settle();
    await settle();
    expect(fakePptxState().destroyed).toBe(1); // 迟到 viewer 立即收口
    expect(view.events).toEqual([]); // 迟火零事件
    expect(view.el.querySelector(".mtdss-preview")).toBeNull();
  });
});

describe("destroy 迟火弃与文案缝", () => {
  it("docx 整取在途 destroy:请求 abort、DOM 摘净、迟火零事件零降级", async () => {
    resetFakeDocx({ pages: 3 });
    const pending = deferred<Response>();
    const fetchMock = vi.fn((_url: unknown, _init?: RequestInit) => pending.promise);
    vi.stubGlobal("fetch", fetchMock);

    const view = mount(officeReq("docx"));
    await settle();
    view.handle.destroy();
    expect(fetchMock.mock.calls[0][1]?.signal?.aborted).toBe(true);
    expect(view.el.querySelector(".mtdss-preview")).toBeNull();

    pending.resolve(fullBytesResponse(BYTES));
    await settle();
    expect(view.events).toEqual([]); // 迟火:渲染与降级皆弃
    expect(view.el.querySelector(".mtdss-preview")).toBeNull();
  });

  it("文案覆写穿透 office 面(loading 文案/两枚续钮)", async () => {
    resetFakeDocx({ pages: 1 });
    sequentialFetch([fullBytesResponse(BYTES)]);
    const view = mount(
      officeReq("docx", { raw: RAW_URL }, "样本.docx", {
        officeLoading: "载入中",
        officeLoadMore: "More",
      }),
    );
    expect(view.loadingNote()?.textContent).toBe("载入中");
    await settleUntil(() => view.moreButton() !== null);
    expect(view.moreButton()?.textContent).toBe("More");
    view.handle.destroy();

    resetFakeXlsx({ sheets: [{ name: "S", rows: sheetRows(600) }] });
    sequentialFetch([fullBytesResponse(BYTES)]);
    const v2 = mount(
      officeReq("xlsx", { raw: RAW_URL }, "样本.xlsx", { xlsxLoadMore: "MoreRows" }),
    );
    await settleUntil(() => v2.tbodyRows().length > 0);
    expect(v2.xlsxMore()?.textContent).toBe("MoreRows");
    v2.handle.destroy();
  });
});
