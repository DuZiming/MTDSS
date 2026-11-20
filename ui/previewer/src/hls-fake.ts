import { vi } from "vitest";

/// hls.js 测试假件(video.ts 唯一消费面;真面行为面=self 事件总线):
/// - 静态面 isSupported() 可配(unsupported 门测试);
/// - 实例面 on/loadSource/attachMedia/startLoad/stopLoad/destroy 记账;
/// - trigger(event, data)= 测试侧驱动事件环(ERROR/FRAG_LOADED/MANIFEST_PARSED)。
/// 常量表键名与真面同源(video.ts 只经 `Hls.Events.*`/`Hls.ErrorDetails.*`
/// 取值,值本身无关紧要——假件自洽即可)。

export interface FakeHlsErrorData {
  fatal: boolean;
  details: string;
  /// 真面 = XHR(缺省 loader)/ fetch Response;测试侧给 XHR 形假件。
  networkDetails?: unknown;
}

export interface FakeHlsInstance {
  config: unknown;
  url: string | null;
  media: unknown;
  destroyed: boolean;
  startLoads: number;
  stopLoads: number;
  on(event: string, fn: (name: string, data: unknown) => void): void;
  loadSource(url: string): void;
  attachMedia(el: unknown): void;
  startLoad(): void;
  stopLoad(): void;
  destroy(): void;
  trigger(event: string, data?: unknown): void;
}

export const fakeHlsState = {
  supported: true,
  instances: [] as FakeHlsInstance[],
  reset(): void {
    this.supported = true;
    this.instances.length = 0;
  },
};

class FakeHls {
  /// 键名与真面同源(真 hls.js Events/ErrorDetails 为 SCREAMING_SNAKE 键,
  /// 值 = hls 内部事件串;video.ts 只经类静态取值,双面自洽)。
  static readonly ErrorDetails = {
    FRAG_LOAD_ERROR: "fragLoadError",
    FRAG_LOAD_TIMEOUT: "fragLoadTimeout",
  };
  static readonly Events = {
    ERROR: "hlsError",
    FRAG_LOADED: "hlsFragLoaded",
    MANIFEST_PARSED: "hlsManifestParsed",
  };
  static isSupported(): boolean {
    return fakeHlsState.supported;
  }

  config: unknown;
  url: string | null = null;
  media: unknown = null;
  destroyed = false;
  startLoads = 0;
  stopLoads = 0;
  private handlers = new Map<string, Set<(name: string, data: unknown) => void>>();

  constructor(config?: unknown) {
    this.config = config;
    fakeHlsState.instances.push(this as unknown as FakeHlsInstance);
  }

  on(event: string, fn: (name: string, data: unknown) => void): void {
    let set = this.handlers.get(event);
    if (!set) {
      set = new Set();
      this.handlers.set(event, set);
    }
    set.add(fn);
  }

  loadSource(url: string): void {
    this.url = url;
  }

  attachMedia(el: unknown): void {
    this.media = el;
  }

  startLoad(): void {
    this.startLoads += 1;
  }

  stopLoad(): void {
    this.stopLoads += 1;
  }

  destroy(): void {
    this.destroyed = true;
  }

  /// 测试驱动:派发事件给已注册处理器(真面 = 内部 loader 回调)。
  trigger(event: string, data?: unknown): void {
    for (const fn of this.handlers.get(event) ?? []) fn(event, data);
  }
}

/// vi.mock("hls.js") 工厂(真面 = default 导出类)。
export function makeFakeHlsModule(): { default: typeof FakeHls } {
  vi.mock; // 引用保持 import 收敛(工厂内不经此引用)
  return { default: FakeHls };
}

/// 503 网络细节假件(XHR 形:status/getResponseHeader/responseText)。
export function xhrLike(
  status: number,
  body: string,
  retryAfter: string | null = null,
): unknown {
  return {
    status,
    responseText: body,
    getResponseHeader: (name: string) =>
      name.toLowerCase() === "retry-after" ? retryAfter : null,
  };
}
