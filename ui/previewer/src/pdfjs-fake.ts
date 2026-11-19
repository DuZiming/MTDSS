/// 假 pdfjs-dist:vi.mock("pdfjs-dist") 工厂 + 模块级记账(每测试 resetFakePdfjs 归零)。
///
/// 仿真契约(对齐 pdf.js v6 真面):
/// - `PDFDataRangeTransport` 基类记账 length/initialData(被测实现子类 override
///   `requestDataRange` 自行 fetch + `onDataRange` 回灌——真 pdf.js 经 getRangeReader 调它);
/// - `getDocument` 速达假 doc(numPages=config);`getPage(n)` 按 rangesPerPage 仿真
///   「pdf.js 需 initialData 之外的字节」——同步经 transport.requestDataRange 取数,
///   被测子类应为此发起 range fetch(请求序列由测试侧 fetch 桩记账);
/// - `page.render({canvas, viewport})` 记账不打真像素;`loadingTask.destroy()` 记账;
/// - worker 面不仿真:被测实现动态 import 的 `pdf.worker.min.mjs` 由测试文件另行 vi.mock 为空模块。

export interface FakePdfjsConfig {
  /// 文档总页数。
  numPages: number;
  /// 页号 → 该页仿真 pdf.js 将发起的 range 请求列 [begin, end](应在 initialData 之外的页)。
  rangesPerPage: Record<number, Array<[number, number]>>;
}

const state = {
  config: { numPages: 9, rangesPerPage: {} } as FakePdfjsConfig,
  /// getDocument 收到的参数(断言 range transport 接线)。
  docParams: [] as Array<Record<string, unknown>>,
  getPageCalls: [] as number[],
  renders: [] as number[],
  /// loadingTask.destroy() 次数(destroy 收口断言)。
  destroyed: 0,
  /// 首枚 transport 构造记账(即被测子类实例本身;length/initialData 字段同断言)。
  transport: null as null | { length: number; initialData: Uint8Array | null },
  /// 经 onDataRange 回灌的 range 交付记账(begin → 已达)。
  delivered: [] as Array<{ begin: number; chunk: Uint8Array | null }>,
};

/// getPage 挂待者(begin → 解析回调);resetFakePdfjs 归零。
const waiters = new Map<number, Array<() => void>>();

export function resetFakePdfjs(numPages: number): void {
  state.config = { numPages, rangesPerPage: {} };
  state.docParams = [];
  state.getPageCalls = [];
  state.renders = [];
  state.destroyed = 0;
  state.transport = null;
  state.delivered = [];
  waiters.clear();
}

/// 为某页配置仿真 range 请求(pdf.js 需要更多字节的面)。
export function fakeRangesFor(page: number, ranges: Array<[number, number]>): void {
  state.config.rangesPerPage[page] = ranges;
}

export function fakePdfjsState(): typeof state {
  return state;
}

type RangeListener = (args: { type: string; begin?: number; chunk?: Uint8Array | null }) => void;

/// vi.mock("pdfjs-dist", () => makeFakePdfjs()) 工厂(可在 vi.mock 回调内动态 import 本文件取用)。
export function makeFakePdfjs(): Record<string, unknown> {
  return {
    version: "6.5.438-fake",
    GlobalWorkerOptions: { workerSrc: "", rendererSrc: "" },
    PDFDataRangeTransport: class FakePDFDataRangeTransport {
      length: number;
      initialData: Uint8Array | null;
      progressiveDone: boolean;
      listener: RangeListener | null = null;
      constructor(length: number, initialData: Uint8Array | null, progressiveDone = false) {
        this.length = length;
        this.initialData = initialData;
        this.progressiveDone = progressiveDone;
        state.transport = this;
      }
      transportReady(listener: RangeListener): void {
        this.listener = listener;
      }
      onDataRange(begin: number, chunk: Uint8Array | null): void {
        state.delivered.push({ begin, chunk });
        this.listener?.({ type: "range", begin, chunk });
        // 交付即唤醒挂待中的 getPage(真面:rangeReader 收字节即续)
        const list = waiters.get(begin);
        if (list !== undefined) {
          waiters.set(begin, []);
          for (const w of [...list]) w();
        }
      }
      onDataProgressiveRead(): void {}
      onDataProgressiveDone(): void {}
      requestDataRange(_begin: number, _end: number): void {
        // 基类即抽象(真面 unreachable);被测子类 override 全接管,此处不应被调。
        throw new Error("FakePDFDataRangeTransport.requestDataRange 未被子类 override");
      }
      abort(): void {}
    },
    getDocument(params: Record<string, unknown>) {
      state.docParams.push(params);
      const transport = params.range as { requestDataRange(begin: number, end: number): void };
      const fakePage = (n: number) => ({
        getViewport({ scale }: { scale: number }) {
          return { width: Math.round(612 * scale), height: Math.round(792 * scale) };
        },
        render() {
          state.renders.push(n);
          return { promise: Promise.resolve() };
        },
      });
      return {
        promise: Promise.resolve({
          numPages: state.config.numPages,
          getPage(n: number) {
            state.getPageCalls.push(n);
            const ranges = state.config.rangesPerPage[n] ?? [];
            if (ranges.length === 0) return Promise.resolve(fakePage(n));
            // 真面仿真:先经 transport 请求字节(getRangeReader → requestDataRange),
            // 字节未交付前页对象不达(render 挂待)——onDataRange 交付即解。
            for (const [b, e] of ranges) transport.requestDataRange(b, e);
            return new Promise((resolve) => {
              const waitAll = (): void => {
                const allDelivered = ranges.every(([b]) =>
                  state.delivered.some((d) => d.begin === b),
                );
                if (allDelivered) resolve(fakePage(n));
              };
              for (const [b] of ranges) {
                let list = waiters.get(b);
                if (list === undefined) {
                  list = [];
                  waiters.set(b, list);
                }
                list.push(waitAll);
              }
              waitAll(); // 已交付过的 range 立即结算
            });
          },
        }),
        destroy() {
          state.destroyed += 1;
          return Promise.resolve();
        },
      };
    },
  };
}
