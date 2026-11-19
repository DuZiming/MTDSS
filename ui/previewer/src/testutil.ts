import { vi, type Mock } from "vitest";

/// 测试共用假件:fetch 短路 / objectURL 记账 / 微任务排空。
/// 实现面纪律:src 只经 globalThis.fetch 取数,URL.createObjectURL/revokeObjectURL
/// 做字节桥——三者均可被本文件桩住,零真实网络、零真实解码。

/// fetch 成功响应形(实现只消费 ok/status/blob)。
export function okResponse(blob: Blob): Response {
  return { ok: true, status: 200, blob: () => Promise.resolve(blob) } as unknown as Response;
}

/// 字节级成功响应形(range 面:实现消费 ok/status/headers.get/arrayBuffer)。
/// content-range 按 begin+data.length 换算,end 由服务端权威钳制(total 为文件全长)。
export function bytesResponse(
  data: Uint8Array,
  init: { begin?: number; total?: number; status?: number } = {},
): Response {
  const begin = init.begin ?? 0;
  const total = init.total ?? begin + data.length;
  const status = init.status ?? 206;
  const end = begin + data.length - 1;
  const headers = new Map([["content-range", `bytes ${begin}-${end}/${total}`]]);
  const body = data.slice(); // 拷贝防测试侧后续改写
  return {
    ok: true,
    status,
    headers: { get: (name: string) => headers.get(String(name).toLowerCase()) ?? null },
    arrayBuffer: () => Promise.resolve(body.buffer as ArrayBuffer),
    blob: () => Promise.resolve(new Blob([body])),
  } as unknown as Response;
}

/// 取 fetch 假件调用实参里的 Range 头(实现面统一走 headers.Range)。
export function rangeHeaderOf(call: unknown[]): string | null {
  const init = call[1] as { headers?: Record<string, string> } | undefined;
  const headers = init?.headers;
  if (!headers) return null;
  return headers["Range"] ?? headers["range"] ?? null;
}

/// 按调用序逐一应答的 fetch 假件(每调用弹一枚;弹尽即拒——多余请求立刻显形)。
export function sequentialFetch(responses: Response[]): Mock {
  const queue = [...responses];
  const mock = vi.fn(() => {
    const res = queue.shift();
    if (res === undefined) return Promise.reject(new Error("unexpected extra fetch"));
    return Promise.resolve(res);
  });
  vi.stubGlobal("fetch", mock);
  return mock;
}

/// fetch 失败响应形(!ok 即按失败臂处理,不读 body)。
export function errResponse(status = 503): Response {
  return { ok: false, status, blob: () => Promise.reject(new Error("no body")) } as unknown as Response;
}

/// 按可变映射路由的 fetch 假件:测试中途改映射即模拟「重试后源恢复」。
export function stubFetch(responses: Record<string, Response>): Mock {
  const mock = vi.fn((url: unknown) => {
    const res = responses[url as string];
    if (res) return Promise.resolve(res);
    return Promise.reject(new Error(`unexpected fetch: ${String(url)}`));
  });
  vi.stubGlobal("fetch", mock);
  return mock;
}

export interface Deferred<T> {
  promise: Promise<T>;
  resolve: (v: T) => void;
}

/// 手控时序的 pending 假件(destroy 中途放弃面:请求永不自答)。
export function deferred<T>(): Deferred<T> {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

export interface ObjectUrlStub {
  /// (blob → url) 创建序,供断言「raw 替换 thumb」时按 blob 反查 url。
  created: Array<{ blob: Blob; url: string }>;
  revoked: string[];
  urlOf(blob: Blob): string;
}

/// URL.createObjectURL/revokeObjectURL 记账桩:字节桥不依赖 happy-dom 解码。
let origCreate: typeof URL.createObjectURL | null = null;
let origRevoke: typeof URL.revokeObjectURL | null = null;

export function stubObjectUrls(): ObjectUrlStub {
  const stub: ObjectUrlStub = {
    created: [],
    revoked: [],
    urlOf(blob: Blob) {
      const hit = this.created.find((c) => c.blob === blob);
      if (!hit) throw new Error("objectURL 从未为该 blob 创建");
      return hit.url;
    },
  };
  let seq = 0;
  origCreate = URL.createObjectURL;
  origRevoke = URL.revokeObjectURL;
  URL.createObjectURL = vi.fn((blob: Blob) => {
    const url = `blob:previewer-test/${++seq}`;
    stub.created.push({ blob, url });
    return url;
  }) as typeof URL.createObjectURL;
  URL.revokeObjectURL = vi.fn((url: string) => {
    stub.revoked.push(url);
  }) as typeof URL.revokeObjectURL;
  return stub;
}

/// 还原全局桩(fetch/objectURL),逐测试隔离。
export function unstubAll(): void {
  vi.unstubAllGlobals();
  if (origCreate !== null) URL.createObjectURL = origCreate;
  if (origRevoke !== null) URL.revokeObjectURL = origRevoke;
  origCreate = null;
  origRevoke = null;
}

/// 排空微任务队列(fetch→blob→setSrc 链深 ~4 微任务;一拍宏任务边界足够)。
export function settle(): Promise<void> {
  return new Promise((r) => setTimeout(r, 0));
}

/// 轮询至谓词成立(动态 import 链路的首次装载耗时跨宏任务;有界等待防挂死)。
export async function settleUntil(pred: () => boolean, timeoutMs = 4000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!pred()) {
    if (Date.now() > deadline) throw new Error("settleUntil 超时:谓词未成立");
    await new Promise((r) => setTimeout(r, 5));
  }
}

export function blob(bytes: number, type = "image/jpeg"): Blob {
  return new Blob([new Uint8Array(bytes)], { type });
}

/// 事件收集:onEvent 假件 → 已发事件数组。
export function eventLog<T>(): { onEvent: (e: T) => void; events: T[] } {
  const events: T[] = [];
  return { onEvent: (e: T) => void events.push(e), events };
}

/// wheel 事件(deltaY<0 放大 / >0 缩小;cancelable 供 preventDefault)。
export function wheelEvent(deltaY: number): WheelEvent {
  return new WheelEvent("wheel", { deltaY, cancelable: true });
}

/// 鼠标 pointer 事件(实现面只吃 pointerType=mouse;触摸留给移动端容器)。
export function mousePointer(
  type: string,
  x: number,
  y: number,
): PointerEvent {
  return new PointerEvent(type, {
    clientX: x,
    clientY: y,
    pointerType: "mouse",
    button: 0,
    cancelable: true,
  });
}

/// 从 img.style.transform("translate(Xpx, Ypx) scale(Z)")读平移/缩放。
export function readTransform(
  img: HTMLImageElement,
): { x: number; y: number; z: number } {
  const m = /translate\((-?[\d.]+)px, (-?[\d.]+)px\) scale\(([\d.]+)\)/.exec(
    img.style.transform,
  );
  if (!m) throw new Error(`transform 形不符:${img.style.transform}`);
  return { x: Number(m[1]), y: Number(m[2]), z: Number(m[3]) };
}

export const THUMB_URL = "http://127.0.0.1:1/f/h/thumb?w=256";
export const RAW_URL = "http://127.0.0.1:1/f/h/raw";
export const JPEG_URL = "http://127.0.0.1:1/f/h/jpeg";
