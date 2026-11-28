/// PDF 查看器(pdf.js + 自定义 range transport,spec §5 渲染器表「先渲首页/前 3 页,
/// 滚动或『查看更多』续取」):
///
/// - 数据面:PdfByteSource 打 urls.raw——首取 PDF_INITIAL_BYTES 窗(≈3 页字节量级,对齐
///   daemon 预读窗 2 MiB;spec「分段参数:PDF 首取 3 页字节;数值收口为常量便于调」);
///   Content-Range 解析总长;续取按 pdf.js 请求的 [begin,end] 精确 range(每请求一枚
///   AbortController,destroy 即断)。
/// - pdf.js 接线:PDFDataRangeTransport 子类——initialData=首取窗、length=总长;
///   requestDataRange=按需 range fetch → onDataRange(begin, chunk) 回灌(真面 pdf.js 经
///   getRangeReader 调用,页对象在所需字节交付前挂待);失败经 onRangeError 钩出重试面。
///   worker 不接:GlobalWorkerOptions.rendererSrc 缺省 → pdf.js 走主线程假工
///   (动态 import worker 模块置 globalThis.pdfjsWorker 即触发,#initialize 首行短路)——
///   零 worker 资产文件,单文件 bundle/WebView 打包友好;渲染走主线程(预览量级可容)。
/// - 渲染面:先渲 PDF_BATCH(3)页(固定 PDF_RENDER_SCALE 位图,CSS 收缩自适应);
///   滚动近底或「查看更多」续取续渲;页码指示进 DOM,meta 事件逐页携 page/pages
///   (PreviewEvent 可选扩展,image 消费者零触碰)。
/// - 失败面:装配/文档级失败=覆盖层+重试钮+retry_needed(重试=整体重启);续载 range
///   失败=同一覆盖层(重试=断点续渲);raw 缺席=终态 error。
/// - destroy:在途 abort、loadingTask destroy、监听摘净、迟火按代次弃。

import { formatPdfPageIndicator } from "./strings";
import type { PreviewStrings } from "./strings";
import { parseRangeTotal } from "./range";
import { pdfWorkerSrc } from "./pdf-worker";
import { pdfWasmParams } from "./pdf-wasm";
import type { PreviewEvent, PreviewHandle, PreviewRequest } from "./types";
import type { PDFDocumentProxy, PDFPageProxy } from "pdfjs-dist";

/// 首取窗:≈3 页字节量级(2 MiB,与 daemon 内存预读窗同量级)。
export const PDF_INITIAL_BYTES = 2 * 1024 * 1024;
/// 首渲页数(spec「先渲首页/前 3 页」)。
export const PDF_BATCH = 3;
/// 滚动近底触发阈(px):距底不足此值即续取。
const PDF_NEAR_BOTTOM_PX = 240;
/// 页位图倍率:固定 2x 位图 + CSS width:100% 收缩(零布局依赖,确定性装配)。
const PDF_RENDER_SCALE = 2;

/// pdf 字节源:range 请求序列执行器(首取窗+按需续取;total 记账)。
class PdfByteSource {
  total: number | null = null;
  private controller: AbortController | null = null;
  private aborted = false;

  constructor(private readonly url: string) {}

  abort(): void {
    this.aborted = true;
    this.controller?.abort();
  }

  /// 首取窗:[0, PDF_INITIAL_BYTES-1](服务端权威钳制,Content-Range 回真实界)。
  async initial(): Promise<Uint8Array> {
    return this.range(0, PDF_INITIAL_BYTES - 1);
  }

  async range(begin: number, end: number): Promise<Uint8Array> {
    if (this.aborted) throw new Error("pdf source aborted");
    const controller = (this.controller = new AbortController());
    const res = await fetch(this.url, {
      headers: { Range: `bytes=${begin}-${end}` },
      signal: controller.signal,
    });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = new Uint8Array(await res.arrayBuffer());
    const headerTotal = parseRangeTotal(res.headers?.get("content-range") ?? null);
    if (headerTotal !== null) {
      this.total = headerTotal;
    } else if (begin === 0) {
      this.total = data.length; // 无 content-range=整取即全量
    }
    return data;
  }
}

type ListenerCleanup = {
  target: EventTarget;
  type: string;
  fn: EventListener;
};

export function mountPdf(
  el: HTMLElement,
  req: PreviewRequest,
  onEvent: (e: PreviewEvent) => void,
  strings: PreviewStrings,
): PreviewHandle {
  const url = req.urls.raw;

  let destroyed = false;
  /// boot 代次:重试/destroy 皆翻代,迟火按代弃。
  let gen = 0;
  let source: PdfByteSource | null = null;
  /// pdf.js loadingTask(destroy 收口 worker/transport)。
  let task: { destroy(): Promise<void> } | null = null;
  let doc: PDFDocumentProxy | null = null;
  let numPages = 0;
  let rendered = 0;
  let inFlight = false;
  let total: number | null = null;

  // ---- 装配:滚动面(逐页画布槽)+ 页码栏(指示+续载钮)+ 失败覆盖层 ----
  const root = document.createElement("div");
  root.className = "mtdss-preview mtdss-preview--pdf";

  const scroller = document.createElement("div");
  scroller.className = "mtdss-preview__pdf-scroll";
  root.appendChild(scroller);

  const bar = document.createElement("div");
  bar.className = "mtdss-preview__bar";
  const indicator = document.createElement("div");
  indicator.className = "mtdss-preview__indicator";
  indicator.hidden = true;
  const moreButton = document.createElement("button");
  moreButton.type = "button";
  moreButton.className = "mtdss-preview__button mtdss-preview__pdf-more";
  moreButton.textContent = strings.pdfLoadMore;
  moreButton.hidden = true;
  bar.append(indicator, moreButton);
  root.appendChild(bar);

  const overlay = document.createElement("div");
  overlay.className = "mtdss-preview__overlay mtdss-preview__overlay--hidden";
  const note = document.createElement("div");
  note.className = "mtdss-preview__note";
  note.textContent = strings.pdfLoadFailed;
  const retryButton = document.createElement("button");
  retryButton.type = "button";
  retryButton.className = "mtdss-preview__button";
  retryButton.textContent = strings.retry;
  overlay.append(note, retryButton);
  root.appendChild(overlay);
  el.appendChild(root);

  // ---- 监听记账:destroy 一次摘净 ----
  const cleanups: ListenerCleanup[] = [];
  const listen = (target: EventTarget, type: string, fn: EventListener): void => {
    target.addEventListener(type, fn);
    cleanups.push({ target, type, fn });
  };

  const showRetry = (): void => {
    overlay.classList.remove("mtdss-preview__overlay--hidden");
  };
  const hideRetry = (): void => {
    overlay.classList.add("mtdss-preview__overlay--hidden");
  };

  const updateBar = (): void => {
    if (numPages === 0) return;
    indicator.textContent = formatPdfPageIndicator(strings.pdfPageIndicator, rendered, numPages);
    indicator.hidden = false;
    moreButton.hidden = rendered >= numPages;
  };

  /// 单页装配:页对象 → viewport(固定 2x)→ canvas 槽 → render;完成后逐页 meta 携页码。
  const renderPage = async (documentProxy: PDFDocumentProxy, pageNo: number, myGen: number): Promise<void> => {
    const page: PDFPageProxy = await documentProxy.getPage(pageNo);
    if (destroyed || gen !== myGen) return;
    const viewport = page.getViewport({ scale: PDF_RENDER_SCALE });
    const canvas = document.createElement("canvas");
    canvas.width = viewport.width;
    canvas.height = viewport.height;
    canvas.className = "mtdss-preview__pdf-canvas";
    const slot = document.createElement("div");
    slot.className = "mtdss-preview__pdf-page";
    slot.appendChild(canvas);
    scroller.appendChild(slot);
    await page.render({ canvas, viewport }).promise;
    if (destroyed || gen !== myGen) return;
    rendered = Math.max(rendered, pageNo);
    onEvent({ type: "meta", bytes: total ?? 0, page: pageNo, pages: numPages });
  };

  /// 续取一页(「查看更多」/滚动近底共用;inFlight 单飞门)。
  const renderNext = async (): Promise<void> => {
    if (destroyed || doc === null || inFlight || rendered >= numPages) return;
    inFlight = true;
    const myGen = gen;
    try {
      await renderPage(doc, rendered + 1, myGen);
      if (destroyed || gen !== myGen) return;
      updateBar();
    } catch {
      if (destroyed || gen !== myGen) return;
      showRetry();
      onEvent({ type: "retry_needed", message: strings.pdfLoadFailed });
    } finally {
      inFlight = false;
    }
  };

  // ---- boot:pdf.js(主线程假工)装载 → 首取窗 → 文档打开 → 先渲 3 页 ----
  const boot = async (): Promise<void> => {
    const myGen = ++gen;
    try {
      // r45 真 worker(blob URL 内嵌原文;pdf.js 自管每文档 worker 生命
      // 周期)——替主线程假工(大 PDF 主线程解析 = UI 冻结白屏根因)
      // r70e legacy 构建:Chromium 83 实测面(modern 构建 ??=/静态块
      // 语法 83 拒解析;legacy 预降级+自带 core-js)。
      const pdfjs = await import("pdfjs-dist/legacy/build/pdf.mjs");
      pdfjs.GlobalWorkerOptions.workerSrc = pdfWorkerSrc();
      if (destroyed || gen !== myGen) return;
      const src = (source = new PdfByteSource(url!));
      const initial = await src.initial();
      if (destroyed || gen !== myGen) return;
      total = src.total ?? initial.length;

      /// 续取 range 失败钩:页对象真面挂待(不拒)——覆盖层由此钩出,重试断点续渲。
      let rangeBroken = false;
      const onRangeError = (reason: unknown): void => {
        void reason;
        if (destroyed || gen !== myGen || rangeBroken) return;
        rangeBroken = true;
        inFlight = false;
        showRetry();
        onEvent({ type: "retry_needed", message: strings.pdfLoadFailed });
      };
      class LoopbackRangeTransport extends pdfjs.PDFDataRangeTransport {
        requestDataRange(begin: number, end: number): void {
          // r46 off-by-one 根修:pdf.js 的 end 是**排他**端点(源码
          // onReceiveData 校验 begin+len ≡ 块界/文件尾),src.range 是含他
          // 语义 → 取 end-1。此前多送一字节 = `Bad end offset` 秒败,
          // 仅 >2MB(首取窗耗尽才发 range)的 PDF 可见——T20 起潜伏。
          void src
            .range(begin, end - 1)
            .then((chunk) => this.onDataRange(begin, chunk), (reason: unknown) => onRangeError(reason));
        }
      }
      const transport = new LoopbackRangeTransport(total, initial, false);

      // r47:wasm 解码器面(JBig2/JPX 扫描件页)——主线程取数形,worker
      // 经 FetchBinaryData 请页面同源 fetch wasmUrl+文件名,字节消息进
      // worker 实例化(blob worker 自取相对路断 = 扫描页空白根因)。
      const loadingTask = pdfjs.getDocument({ range: transport, ...pdfWasmParams() });
      task = loadingTask;
      const documentProxy = await loadingTask.promise;
      if (destroyed || gen !== myGen) return;
      doc = documentProxy;
      numPages = documentProxy.numPages;
      rendered = 0;
      hideRetry();
      for (let i = 1; i <= Math.min(PDF_BATCH, numPages); i += 1) {
        await renderPage(documentProxy, i, myGen);
        if (destroyed || gen !== myGen) return;
      }
      updateBar();
    } catch {
      if (destroyed || gen !== myGen) return;
      showRetry();
      onEvent({ type: "retry_needed", message: strings.pdfLoadFailed });
    }
  };

  listen(moreButton, "click", () => {
    void renderNext();
  });
  listen(retryButton, "click", () => {
    if (destroyed) return;
    hideRetry();
    if (doc !== null) {
      gen += 1; // 弃前次挂待页(真面 range 失败页对象不达不拒;迟火按代弃)
      void renderNext(); // 续载失败:断点续渲
    } else {
      void boot(); // 文档级失败:整体重启
    }
  });
  listen(scroller, "scroll", () => {
    if (doc === null || inFlight || rendered >= numPages) return;
    const nearBottom =
      scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - PDF_NEAR_BOTTOM_PX;
    if (nearBottom) void renderNext();
  });

  // ---- boot 入口 ----
  if (url === undefined) {
    // 装配即不可预览:终态 error,无重试面
    retryButton.remove();
    overlay.classList.remove("mtdss-preview__overlay--hidden");
    onEvent({ type: "error", message: strings.unavailableMessage });
  } else {
    void boot();
  }

  // ---- destroy:监听摘净 + 在途 abort + loadingTask 收口 + 迟火弃 ----
  return {
    destroy: () => {
      if (destroyed) return;
      destroyed = true;
      gen += 1;
      source?.abort();
      void task?.destroy().catch(() => {});
      for (const entry of [...cleanups]) {
        entry.target.removeEventListener(entry.type, entry.fn);
      }
      root.remove();
    },
  };
}
