/// office 查看器(docx/xlsx/pptx 共骨架,spec §5 渲染器表):
///
/// - 数据面:「整取」= 单枚无 Range 请求打 urls.raw(隐式全量 range/200;spec「后台整取后
///   逐页渲」)——静默后台拉全文件,无下载动作、不进任务列表;拉取/解析期间出加载面
///   (spinner+officeLoading 文案缝);
/// - docx:docx-preview parseAsync+renderDocument——inWrapper:false=逐页拆节点(先渲
///   OFFICE_BATCH(3)页,「查看更多」/滚动近底逐段续渲);renderAltChunks:false=从源头摘除
///   altChunk 的 iframe.srcdoc 原始 HTML 通路(消毒纪律:office 库输出是自建 DOM/受控结构,
///   直挂容器;原始 HTML 通路一律摘除而非消毒后放行);
/// - xlsx:SheetJS CE read+sheet_to_json({header:1, raw:false, defval:""} 字符串化读面)→
///   轻量表格(textContent 单点装配,零 HTML 解析面):sheet 标签切换(窗重置)+首屏
///   XLSX_WINDOW_ROWS(500)行,「加载更多」/滚动近底续行;
/// - pptx:@file-viewer/pptx PptxViewer——windowed 滚动窗(listOptions.initialSlides=
///   OFFICE_BATCH 钉「先渲前 3 页」,续页由 viewer 自身滚动窗展开);解析 worker 经
///   ./pptx-worker 内联打包口注入(单文件资产纪律);无自建续钮(viewer 即交互面);
/// - 失败面:整取/解析/渲染失败(含零可渲染内容)一律降级 unsupported 形——复用 router
///   占位面板单点(unsupported.ts:文件名+unsupportedNote+下载引导,文案缝原词)+
///   officeLoadFailed 终态 error 事件(无重试面);
/// - 预读帽:meta.size > OFFICE_MAX_BYTES(256MB,同 T6 jpeg 预读 size 快照帽)装配即降级
///   零 fetch;响应 Content-Length 声明超帽同拒(二道门)——用户裁定③「不设单文件上限」
///   归预览缓存落盘面,非无界内存读;
/// - pptx worker 面:worker 创建/运行失败(PPTX_WORKER_FAILED)降级携 officeWorkerFailed
///   专属事件词+console 记账(容器 CSP 须放行 worker-src blob:,见 pptx-worker.ts);
/// - destroy:在途 abort、viewer destroy 收口(迟到 viewer 到达即收口)、监听摘净、迟火按
///   代次弃(gen 房法沿 T17;降级后 destroy 连带占位面板摘净)。

import type { PreviewStrings } from "./strings";
import { mountUnsupported } from "./unsupported";
import { fetchWithTimeout } from "./fetch-timeout";
import type { PreviewEvent, PreviewHandle, PreviewRequest } from "./types";

/// docx 先渲页数/pptx 滚动窗首屏页数(spec「office 首渲 3 页」)。
export const OFFICE_BATCH = 3;
/// xlsx 首屏/每窗行数(spec「整取后按 sheet 渲首屏行(500 行)」)。
export const XLSX_WINDOW_ROWS = 500;
/// 整取预读帽:256MB(与 T6 jpeg 预读 size 快照帽同值对齐)——帽上文件在解析前即降级
/// unsupported(下载引导在),不让 GB 级文件整进 RAM。
export const OFFICE_MAX_BYTES = 256 * 1024 * 1024;
/// 滚动近底触发阈(px;pdf 查看器同值)。
const OFFICE_NEAR_BOTTOM_PX = 240;

/// 整取:单枚全量 GET(无 Range 头;服务端隐式全量 range/200 回全量字节)。
/// Content-Length 二道门:响应声明超帽即拒读 body(缺头不拦——一道门 meta.size 已拦诚实容器)。
async function fetchWholeFile(url: string, signal: AbortSignal): Promise<Uint8Array> {
  // r70g field #1:取数看门狗(远端拨号挂起 = 永黑/白根因;同 pdf/text)
  const res = await fetchWithTimeout(url, { signal });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new Error(`HTTP ${res.status} ${body.slice(0, 160)}`);
  }
  if (declaredTooBig(res)) throw new Error("office 整取超预读帽(Content-Length)");
  return new Uint8Array(await res.arrayBuffer());
}

function declaredTooBig(res: Response): boolean {
  const declared = res.headers?.get("content-length");
  if (declared === null) return false;
  const n = Number(declared);
  return Number.isFinite(n) && n > OFFICE_MAX_BYTES;
}

type ListenerCleanup = {
  target: EventTarget;
  type: string;
  fn: EventListener;
};

/// office 共骨架:root(加载面覆盖 + 滚动面 + 续钮栏)。moreLabel=null=无续钮(pptx,
/// viewer 自身滚动窗即续取面)。
interface OfficeShell {
  root: HTMLDivElement;
  scroller: HTMLDivElement;
  loading: HTMLDivElement;
  bar: HTMLDivElement | null;
  moreButton: HTMLButtonElement | null;
}

function buildShell(strings: PreviewStrings, opts: {
  moreLabel: string | null;
  moreClass: string;
  scrollerClass: string;
}): OfficeShell {
  const root = document.createElement("div");
  root.className = "mtdss-preview mtdss-preview--office";

  const loading = document.createElement("div");
  loading.className = "mtdss-preview__office-loading";
  const spinner = document.createElement("div");
  spinner.className = "mtdss-preview__spinner";
  const note = document.createElement("div");
  note.className = "mtdss-preview__note";
  note.textContent = strings.officeLoading;
  loading.append(spinner, note);

  const scroller = document.createElement("div");
  scroller.className = opts.scrollerClass;

  root.append(loading, scroller);

  let bar: HTMLDivElement | null = null;
  let moreButton: HTMLButtonElement | null = null;
  if (opts.moreLabel !== null) {
    bar = document.createElement("div");
    bar.className = "mtdss-preview__bar";
    bar.hidden = true; // 首批就绪前不显
    moreButton = document.createElement("button");
    moreButton.type = "button";
    moreButton.className = `mtdss-preview__button ${opts.moreClass}`;
    moreButton.textContent = opts.moreLabel;
    moreButton.hidden = true;
    bar.appendChild(moreButton);
    root.appendChild(bar);
  }
  return { root, scroller, loading, bar, moreButton };
}

/// 降级单点:解析/渲染失败 → unsupported 形(router 占位面板复用)+终态 error 事件。
/// 调用方须先摘除 office DOM(root.remove)再 degrade——占位面板与 office 面不共存。
/// message 缺省=officeLoadFailed;pptx worker 面传 officeWorkerFailed(专属词,集成可辨)。
function createDegradation(
  el: HTMLElement,
  req: PreviewRequest,
  strings: PreviewStrings,
  onEvent: (e: PreviewEvent) => void,
) {
  let degraded = false;
  let placeholder: PreviewHandle | null = null;
  return {
    isDegraded: (): boolean => degraded,
    degrade: (message?: string, detail?: string): void => {
      if (degraded) return;
      degraded = true;
      placeholder = mountUnsupported(el, req, strings);
      // field 诊断(2026-11-22):真实错误透出——pptx「久等后不支持」等
      // 无控制台红字的失败面,原因写进占位面板(用户/现场可读)。
      if (detail !== undefined && detail !== "") {
        const host = el.lastElementChild;
        if (host !== null) {
          const d = document.createElement("div");
          d.className = "mtdss-preview__note";
          d.style.opacity = "0.7";
          d.textContent = detail.slice(0, 300);
          host.appendChild(d);
        }
      }
      onEvent({ type: "error", message: message ?? strings.officeLoadFailed, detail });
    },
    destroyPlaceholder: (): void => {
      placeholder?.destroy();
      placeholder = null;
    },
  };
}

/// field 修复(2026-11-22):错误面文本化——非 Error 对象(pptx viewer 的
/// onError 载荷是普通对象)String() 只出 "[object Object]",改 JSON 探底。
function errText(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  try {
    return JSON.stringify(err) ?? String(err);
  } catch {
    return String(err);
  }
}

/// PptxDiagnosticError 形的 code 抽取(viewer 面错误分类单点;非诊断形 → null)。
function pptxDiagCode(err: unknown): string | null {
  return typeof err === "object" && err !== null &&
    typeof (err as { code?: unknown }).code === "string"
    ? (err as { code: string }).code
    : null;
}

/// 续钮/续钮栏状态窗面(docx/xlsx 共面):页尽即整栏隐没(bar 只承载续钮,不留死空条);
/// xlsx 切 sheet 回未尽窗时同门复位(钮恒在 DOM,hidden 翻转,免拆建 resurrection 面)。
function updateMoreFace(shell: OfficeShell, exhausted: boolean): void {
  if (shell.moreButton !== null) shell.moreButton.hidden = exhausted;
  if (shell.bar !== null) shell.bar.hidden = exhausted;
}

/// docx 查看器(docx-preview)。
export function mountDocx(
  el: HTMLElement,
  req: PreviewRequest,
  onEvent: (e: PreviewEvent) => void,
  strings: PreviewStrings,
): PreviewHandle {
  const url = req.urls.raw;
  const degradation = createDegradation(el, req, strings, onEvent);

  let destroyed = false;
  /// 取数代次:destroy 翻代,迟火按代弃。
  let gen = 0;
  let abortCtl: AbortController | null = null;
  const cleanups: ListenerCleanup[] = [];
  const listen = (target: EventTarget, type: string, fn: EventListener): void => {
    target.addEventListener(type, fn);
    cleanups.push({ target, type, fn });
  };

  const shell = buildShell(strings, {
    moreLabel: strings.officeLoadMore,
    moreClass: "mtdss-preview__office-more",
    scrollerClass: "mtdss-preview__office-scroll",
  });
  /// 库样式箱:docx-preview 产出的 STYLE 节点容器(随 root 销毁,不污文档头)。
  const styleBox = document.createElement("div");
  styleBox.className = "mtdss-preview__office-styles";
  shell.root.insertBefore(styleBox, shell.scroller);
  el.appendChild(shell.root);

  /// 页节点账(renderDocument 产出):STYLE 节点进样式箱,页节点按批进滚动面。
  const pages: Node[] = [];
  let shown = 0;
  let bytes = 0;

  const revealNext = (): void => {
    if (shown >= pages.length) return;
    const end = Math.min(shown + OFFICE_BATCH, pages.length);
    for (; shown < end; shown += 1) {
      const slot = document.createElement("div");
      slot.className = "mtdss-preview__office-page";
      slot.appendChild(pages[shown]);
      shell.scroller.appendChild(slot);
    }
    updateMoreFace(shell, shown >= pages.length);
    onEvent({ type: "meta", bytes, page: shown, pages: pages.length });
  };

  const boot = async (): Promise<void> => {
    const myGen = ++gen;
    try {
      const controller = (abortCtl = new AbortController());
      const data = await fetchWholeFile(url!, controller.signal);
      if (destroyed || gen !== myGen) return;
      bytes = data.length;
      const docx = await import("docx-preview");
      if (destroyed || gen !== myGen) return;
      const doc = await docx.parseAsync(data, { inWrapper: false, renderAltChunks: false });
      if (destroyed || gen !== myGen) return;
      const nodes = await docx.renderDocument(doc);
      if (destroyed || gen !== myGen) return;
      for (const n of nodes) {
        if (n.nodeName === "STYLE") styleBox.appendChild(n);
        else pages.push(n);
      }
      if (pages.length === 0) throw new Error("docx 无可渲染页");
      shell.loading.remove();
      shell.bar!.hidden = false;
      revealNext();
    } catch {
      if (destroyed || gen !== myGen || degradation.isDegraded()) return;
      shell.root.remove();
      degradation.degrade();
    }
  };

  listen(shell.moreButton!, "click", () => {
    if (destroyed) return;
    revealNext();
  });
  listen(shell.scroller, "scroll", () => {
    if (destroyed || shown >= pages.length) return;
    const nearBottom =
      shell.scroller.scrollTop + shell.scroller.clientHeight >=
      shell.scroller.scrollHeight - OFFICE_NEAR_BOTTOM_PX;
    if (nearBottom) revealNext();
  });

  if (url === undefined || req.meta.size > OFFICE_MAX_BYTES) {
    // 装配即不可预览(raw 缺/超预读帽):终态降级,零 fetch
    shell.root.remove();
    degradation.degrade();
  } else {
    void boot();
  }

  return {
    destroy: () => {
      if (destroyed) return;
      destroyed = true;
      gen += 1;
      abortCtl?.abort();
      degradation.destroyPlaceholder();
      for (const entry of [...cleanups]) {
        entry.target.removeEventListener(entry.type, entry.fn);
      }
      shell.root.remove();
    },
  };
}

/// xlsx 查看器(SheetJS CE + 轻量表格渲染)。
export function mountXlsx(
  el: HTMLElement,
  req: PreviewRequest,
  onEvent: (e: PreviewEvent) => void,
  strings: PreviewStrings,
): PreviewHandle {
  const url = req.urls.raw;
  const degradation = createDegradation(el, req, strings, onEvent);

  let destroyed = false;
  let gen = 0;
  let abortCtl: AbortController | null = null;
  const cleanups: ListenerCleanup[] = [];
  const listen = (target: EventTarget, type: string, fn: EventListener): void => {
    target.addEventListener(type, fn);
    cleanups.push({ target, type, fn });
  };

  const shell = buildShell(strings, {
    moreLabel: strings.xlsxLoadMore,
    moreClass: "mtdss-preview__xlsx-more",
    scrollerClass: "mtdss-preview__sheet-scroll",
  });
  const tabs = document.createElement("div");
  tabs.className = "mtdss-preview__sheet-tabs";
  shell.root.insertBefore(tabs, shell.scroller);
  el.appendChild(shell.root);

  const table = document.createElement("table");
  table.className = "mtdss-preview__sheet-table";
  const thead = document.createElement("thead");
  const tbody = document.createElement("tbody");
  table.append(thead, tbody);
  shell.scroller.appendChild(table);

  let sheets: Array<{ name: string; rows: unknown[][] }> = [];
  let active = 0;
  /// 已上屏行数(含表头行;窗=XLSX_WINDOW_ROWS,「加载更多」逐窗推进)。
  let shown = 0;
  let bytes = 0;

  /// 单元格字符串化(textContent 单点;零 HTML 解析面,数字/富值一律 String 化)。
  const cellText = (v: unknown): string => (v === null || v === undefined ? "" : String(v));

  const fillRow = (row: unknown[], tag: "td" | "th"): HTMLTableRowElement => {
    const tr = document.createElement("tr");
    for (const cell of row) {
      const c = document.createElement(tag);
      c.textContent = cellText(cell);
      tr.appendChild(c);
    }
    return tr;
  };

  /// 体行装配:[from, to) 行(rows[0] 为表头不在其列)。
  const appendBodyRows = (from: number, to: number): void => {
    const rows = sheets[active].rows;
    for (let i = Math.max(from, 1); i < to; i += 1) {
      tbody.appendChild(fillRow(rows[i] ?? [], "td"));
    }
  };

  const metaEmit = (): void => {
    onEvent({ type: "meta", bytes, page: shown, pages: sheets[active].rows.length });
  };

  /// 激活 sheet 整面重建(thead=首行,tbody=窗内体行;切换 sheet 即窗重置)。
  const renderSheet = (): void => {
    const rows = sheets[active].rows;
    shown = Math.min(XLSX_WINDOW_ROWS, rows.length);
    thead.replaceChildren();
    tbody.replaceChildren();
    if (rows.length > 0) thead.appendChild(fillRow(rows[0], "th"));
    appendBodyRows(1, shown);
    updateMoreFace(shell, shown >= rows.length);
    metaEmit();
  };

  const loadMore = (): void => {
    const rows = sheets[active].rows;
    if (shown >= rows.length) return;
    const next = Math.min(shown + XLSX_WINDOW_ROWS, rows.length);
    appendBodyRows(shown, next);
    shown = next;
    updateMoreFace(shell, shown >= rows.length);
    metaEmit();
  };

  const buildTabs = (): void => {
    sheets.forEach((sheet, i) => {
      const tab = document.createElement("button");
      tab.type = "button";
      tab.className = `mtdss-preview__sheet-tab${i === active ? " mtdss-preview__sheet-tab--active" : ""}`;
      tab.textContent = sheet.name;
      listen(tab, "click", () => {
        if (destroyed || i === active) return;
        active = i;
        for (const [j, t] of [...tabs.children].entries()) {
          t.classList.toggle("mtdss-preview__sheet-tab--active", j === active);
        }
        renderSheet();
      });
      tabs.appendChild(tab);
    });
  };

  const boot = async (): Promise<void> => {
    const myGen = ++gen;
    try {
      const controller = (abortCtl = new AbortController());
      const data = await fetchWholeFile(url!, controller.signal);
      if (destroyed || gen !== myGen) return;
      bytes = data.length;
      const XLSX = await import("xlsx");
      if (destroyed || gen !== myGen) return;
      const wb = XLSX.read(data, { type: "array" });
      if (destroyed || gen !== myGen) return;
      sheets = wb.SheetNames.map((name) => ({
        name,
        rows: XLSX.utils.sheet_to_json(wb.Sheets[name], {
          header: 1,
          raw: false,
          defval: "",
        }) as unknown[][],
      }));
      if (sheets.length === 0) throw new Error("xlsx 无工作表");
      shell.loading.remove();
      buildTabs();
      renderSheet();
      shell.bar!.hidden = false;
    } catch {
      if (destroyed || gen !== myGen || degradation.isDegraded()) return;
      shell.root.remove();
      degradation.degrade();
    }
  };

  listen(shell.moreButton!, "click", () => {
    if (destroyed) return;
    loadMore();
  });
  listen(shell.scroller, "scroll", () => {
    if (destroyed || sheets.length === 0) return;
    const rows = sheets[active].rows;
    if (shown >= rows.length) return;
    const nearBottom =
      shell.scroller.scrollTop + shell.scroller.clientHeight >=
      shell.scroller.scrollHeight - OFFICE_NEAR_BOTTOM_PX;
    if (nearBottom) loadMore();
  });

  if (url === undefined || req.meta.size > OFFICE_MAX_BYTES) {
    shell.root.remove();
    degradation.degrade();
  } else {
    void boot();
  }

  return {
    destroy: () => {
      if (destroyed) return;
      destroyed = true;
      gen += 1;
      abortCtl?.abort();
      degradation.destroyPlaceholder();
      for (const entry of [...cleanups]) {
        entry.target.removeEventListener(entry.type, entry.fn);
      }
      shell.root.remove();
    },
  };
}

/// pptx 查看器(@file-viewer/pptx PptxViewer)。
/// viewer 需要的最小形(结构化子集;真面 PptxViewer 满足)。
interface PptxViewerLike {
  slideCount: number;
  destroy(): void;
}

export function mountPptx(
  el: HTMLElement,
  req: PreviewRequest,
  onEvent: (e: PreviewEvent) => void,
  strings: PreviewStrings,
): PreviewHandle {
  const url = req.urls.raw;
  const degradation = createDegradation(el, req, strings, onEvent);

  let destroyed = false;
  let gen = 0;
  let abortCtl: AbortController | null = null;
  const cleanups: ListenerCleanup[] = [];

  // pptx 无自建续钮:viewer windowed 滚动窗(initialSlides=3)先渲 3 页,滚动即续页
  const shell = buildShell(strings, {
    moreLabel: null,
    moreClass: "",
    scrollerClass: "mtdss-preview__office-scroll",
  });
  const target = document.createElement("div");
  target.className = "mtdss-preview__office-pptx";
  shell.scroller.appendChild(target);
  el.appendChild(shell.root);

  let viewer: PptxViewerLike | null = null;
  let rendered = 0;
  let bytes = 0;
  let firstMeta = false;

  const onSlideRendered = (): void => {
    if (destroyed || degradation.isDegraded()) return;
    rendered += 1;
    if (!firstMeta) {
      // 主内容就绪面:首枚幻灯片上屏即发(此时 viewer.slideCount 已随 worker 消息就位)
      firstMeta = true;
      shell.loading.remove();
      onEvent({ type: "meta", bytes, page: rendered, pages: viewer?.slideCount ?? 0 });
    }
  };

  const onComplete = (): void => {
    if (destroyed || degradation.isDegraded()) return;
    shell.loading.remove();
    if (rendered === 0) {
      // 渲染完零可上屏内容:降级(spec「渲染失败=类型页+下载引导」)
      shell.root.remove();
      degradation.degrade();
      return;
    }
    onEvent({ type: "meta", bytes, page: rendered, pages: viewer?.slideCount ?? rendered });
  };

  /// worker 面失败(PPTX_WORKER_FAILED):降级携 officeWorkerFailed 专属事件词+console
  /// 记账——十有八九=容器 CSP 未放行 worker-src blob:(见 pptx-worker.ts),集成按词识别。
  const degradeWorkerFace = (err: unknown): void => {
    console.warn("[mtdss-preview] pptx worker 不可用:请检查容器 CSP worker-src blob:", err);
    shell.root.remove();
    degradation.degrade(strings.officeWorkerFailed);
  };

  const onPptxError = (err: unknown): void => {
    if (destroyed || degradation.isDegraded()) return;
    if (pptxDiagCode(err) === "PPTX_WORKER_FAILED") {
      degradeWorkerFace(err);
      return;
    }
    const detail = errText(err);
    shell.root.remove();
    degradation.degrade(undefined, `pptx runtime: ${detail}`);
  };

  const boot = async (): Promise<void> => {
    const myGen = ++gen;
    try {
      const controller = (abortCtl = new AbortController());
      // r70g field #1:取数看门狗(同 fetchWholeFile)
      const res = await fetchWithTimeout(url!, { signal: controller.signal });
      if (destroyed || gen !== myGen) return;
      if (!res.ok) {
        const body = await res.text().catch(() => "");
        throw new Error(`HTTP ${res.status} ${body.slice(0, 160)}`);
      }
      if (declaredTooBig(res)) throw new Error("office 整取超预读帽(Content-Length)");
      const buffer = await res.arrayBuffer();
      if (destroyed || gen !== myGen) return;
      bytes = buffer.byteLength;
      const [pptx, workerMod] = await Promise.all([
        import("@file-viewer/pptx"),
        import("./pptx-worker"),
      ]);
      if (destroyed || gen !== myGen) return;
      const opened = await pptx.PptxViewer.open(buffer, target, {
        workerFactory: workerMod.createPptxParserWorker,
        listOptions: { windowed: true, initialSlides: OFFICE_BATCH, batchSize: OFFICE_BATCH },
        onSlideRendered: () => onSlideRendered(),
        onRenderComplete: () => onComplete(),
        onError: (err) => onPptxError(err),
      });
      viewer = opened;
      if (destroyed || gen !== myGen) {
        // 迟到的 viewer(open 达前已 destroy):立即收口,不留活 worker
        opened.destroy();
        return;
      }
    } catch (err) {
      const detail = errText(err);
      if (destroyed || gen !== myGen || degradation.isDegraded()) return;
      if (pptxDiagCode(err) === "PPTX_WORKER_FAILED") {
        degradeWorkerFace(err);
        return;
      }
      shell.root.remove();
      degradation.degrade(undefined, `pptx: ${detail}`);
    }
  };

  if (url === undefined || req.meta.size > OFFICE_MAX_BYTES) {
    shell.root.remove();
    degradation.degrade();
  } else {
    void boot();
  }

  return {
    destroy: () => {
      if (destroyed) return;
      destroyed = true;
      gen += 1;
      abortCtl?.abort();
      viewer?.destroy();
      degradation.destroyPlaceholder();
      for (const entry of [...cleanups]) {
        entry.target.removeEventListener(entry.type, entry.fn);
      }
      shell.root.remove();
    },
  };
}
