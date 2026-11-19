/// 假 office 库三件:vi.mock("docx-preview"/"xlsx"/"@file-viewer/pptx") 工厂 + 模块级记账
/// (每测试 resetFakeDocx/resetFakeXlsx/resetFakePptx 归零)。零真实解析、零真实 worker。
///
/// 仿真契约(对齐各库真面):
/// - docx-preview:`parseAsync(data, opts)` 记账(字节量/opts 形,可配抛错)→
///   `renderDocument(doc)` 产出 [STYLE 节点 + 每页一枚 section.docx](真面 inWrapper:false
///   节点形同构:样式 STYLE 节点在前、页节点随后);
/// - xlsx(SheetJS):`read(data, opts)` 记账(可配抛错)→ Workbook 形
///   {SheetNames, Sheets};`utils.sheet_to_json(ws, opts)` 回测试配置的 rows 二维表;
/// - @file-viewer/pptx:`PptxViewer.open(buffer, target, options)` 记账(buffer/target/options)
///   ——open 返回的 promise 在微任务后才触发回调(真面=worker 消息流在 open() 达后异步到):
///   先逐枚追加 slide 子元素并回调 onSlideRendered,末枚 onRenderComplete(或可配 onError/
///   open 拒/挂起待测试手放);destroy 记账。

export interface FakeDocxConfig {
  /// 文档页数(renderDocument 产出的 section 节点数)。
  pages: number;
  /// parseAsync 抛错(解析失败降级钉面)。
  failParse?: boolean;
  /// renderDocument 抛错(渲染失败降级钉面)。
  failRender?: boolean;
}

const docxState = {
  config: { pages: 0 } as FakeDocxConfig,
  parseCalls: [] as Array<{ byteLength: number; opts: Record<string, unknown> | undefined }>,
  renderCalls: 0,
};

export function resetFakeDocx(config: FakeDocxConfig): void {
  docxState.config = { ...config };
  docxState.parseCalls = [];
  docxState.renderCalls = 0;
}

export function fakeDocxState(): typeof docxState {
  return docxState;
}

export function makeFakeDocxPreview(): Record<string, unknown> {
  return {
    parseAsync: async (data: { byteLength?: number }, opts?: Record<string, unknown>) => {
      docxState.parseCalls.push({ byteLength: data?.byteLength ?? 0, opts });
      if (docxState.config.failParse) throw new Error("fake docx parse failed");
      return {};
    },
    renderDocument: async (): Promise<Node[]> => {
      docxState.renderCalls += 1;
      if (docxState.config.failRender) throw new Error("fake docx render failed");
      const nodes: Node[] = [document.createElement("style")];
      for (let i = 0; i < docxState.config.pages; i += 1) {
        const section = document.createElement("section");
        section.className = "docx";
        nodes.push(section);
      }
      return nodes;
    },
  };
}

export interface FakeSheet {
  name: string;
  /// 二维表(header:1 形;含首行表头)。
  rows: unknown[][];
}

export interface FakeXlsxConfig {
  sheets: FakeSheet[];
  /// read 抛错(解析失败降级钉面)。
  failParse?: boolean;
}

const xlsxState = {
  config: { sheets: [] } as FakeXlsxConfig,
  readCalls: [] as Array<{ byteLength: number; opts: Record<string, unknown> | undefined }>,
  toJsonOpts: [] as Array<Record<string, unknown> | undefined>,
};

export function resetFakeXlsx(config: FakeXlsxConfig): void {
  xlsxState.config = { sheets: config.sheets, failParse: config.failParse };
  xlsxState.readCalls = [];
  xlsxState.toJsonOpts = [];
}

export function fakeXlsxState(): typeof xlsxState {
  return xlsxState;
}

export function makeFakeXlsx(): Record<string, unknown> {
  return {
    read: (data: { byteLength?: number }, opts?: Record<string, unknown>) => {
      xlsxState.readCalls.push({ byteLength: data?.byteLength ?? 0, opts });
      if (xlsxState.config.failParse) throw new Error("fake xlsx parse failed");
      const sheets: Record<string, unknown> = {};
      for (const s of xlsxState.config.sheets) {
        sheets[s.name] = { __rows: s.rows, "!ref": "A1" };
      }
      return {
        SheetNames: xlsxState.config.sheets.map((s) => s.name),
        Sheets: sheets,
      };
    },
    utils: {
      sheet_to_json: (ws: { __rows?: unknown[][] }, opts?: Record<string, unknown>) => {
        xlsxState.toJsonOpts.push(opts);
        return ws?.__rows ?? [];
      },
    },
  };
}

export interface FakePptxConfig {
  /// 幻灯片数(open 后逐枚 onSlideRendered)。
  slides: number;
  /// open 拒(worker 起不来等装配面失败)。
  failOpen?: boolean;
  /// 渲染中途 onError(文档级渲染失败)。
  failRender?: boolean;
  /// open 挂起不答(测试经 pptxState.openGate 手放;destroy 迟火钉面)。
  deferOpen?: boolean;
}

/// 被测实现传入 open 的 options 形(回调面+记账面;余键透传 Record)。
export interface FakePptxOptions {
  onSlideRendered?: (slideNumber: number, element: Element | null) => void;
  onRenderComplete?: () => void;
  onError?: (error: unknown) => void;
  listOptions?: Record<string, unknown>;
  workerFactory?: unknown;
  [key: string]: unknown;
}

const pptxState = {
  config: { slides: 0 } as FakePptxConfig,
  opens: [] as Array<{
    byteLength: number;
    targetClass: string | null;
    options: FakePptxOptions;
  }>,
  slidesMounted: 0,
  destroyed: 0,
  /// deferOpen 形的放开器(测试 resolve 一枚 viewer 形对象)。
  openGate: null as null | { resolve: (viewer: Record<string, unknown>) => void },
};

export function resetFakePptx(config: FakePptxConfig): void {
  pptxState.config = { ...config };
  pptxState.opens = [];
  pptxState.slidesMounted = 0;
  pptxState.destroyed = 0;
  pptxState.openGate = null;
}

export function fakePptxState(): typeof pptxState {
  return pptxState;
}

export function makeFakePptx(): Record<string, unknown> {
  return {
    PptxViewer: class FakePptxViewer {
      static open(
        buffer: ArrayBuffer,
        target: HTMLElement,
        options: FakePptxOptions,
      ): Promise<Record<string, unknown>> {
        pptxState.opens.push({
          byteLength: buffer?.byteLength ?? 0,
          targetClass: target.className,
          options,
        });
        if (pptxState.config.failOpen) {
          return Promise.reject(new Error("fake pptx open failed"));
        }
        if (pptxState.config.deferOpen) {
          return new Promise((resolve) => {
            pptxState.openGate = { resolve };
          });
        }
        return new Promise((resolve) => {
          // open() 达(worker 消息异步到):先解 viewer,下一拍再走消息流回调
          setTimeout(() => {
            const viewer = {
              slideCount: pptxState.config.slides,
              destroy(): void {
                pptxState.destroyed += 1;
              },
            };
            resolve(viewer);
            setTimeout(() => {
              if (pptxState.config.failRender) {
                // 文档级渲染失败(worker 消息流报错,零幻灯片上屏):单枚 onError
                options.onError?.({ name: "PptxDiagnosticError", code: "PPTX_INVALID_ZIP" });
                return;
              }
              for (let i = 1; i <= pptxState.config.slides; i += 1) {
                const slide = document.createElement("div");
                slide.className = "fv-slide";
                target.appendChild(slide);
                pptxState.slidesMounted += 1;
                options.onSlideRendered?.(i, slide);
              }
              options.onRenderComplete?.();
            }, 0);
          }, 0);
        });
      }
    },
  };
}
