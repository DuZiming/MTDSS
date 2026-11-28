/// 旧引擎运行时内建垫(r70e):esbuild target 只降**语法**(??=/静态块等),
/// 不降运行时**内建方法**——供应商码(markdown-it/highlight.js/dompurify/
/// hls.js 等)用到的少量 85+/9x 内建在旧 Android WebView(实测 Chromium 83,
/// MIX3 实机 dumpsys)缺席即 TypeError。新引擎 `if (!x)` 全短路,零开销零
/// 行为变化;pdf.js 走 legacy 构建(自带 core-js 垫片),不经此面。
///
/// 纪律:只垫**确证被用到的**(r70d 产物 grep 计数:replaceAll×14/.at×18/
/// Object.hasOwn×3/structuredClone×5/findLast×1);不垫整包 core-js——
/// 逐枚垫,面小可审,缺了再补。

/* eslint-disable @typescript-eslint/no-unnecessary-condition */

const arrProto = Array.prototype as unknown as Record<string, unknown>;
const strProto = String.prototype as unknown as Record<string, unknown>;

if (typeof strProto.replaceAll !== "function") {
  /// replaceAll 字符串搜索形:转全局正则走 replace($& 等替换模式语义保留)。
  strProto.replaceAll = function (
    this: string,
    search: string,
    replacement: string,
  ): string {
    const esc = search.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    return this.replace(new RegExp(esc, "g"), replacement);
  };
}

if (typeof arrProto.at !== "function") {
  arrProto.at = function (this: unknown[], i: number): unknown {
    const n = Math.trunc(i) || 0;
    const idx = n < 0 ? this.length + n : n;
    return idx < 0 || idx >= this.length ? undefined : this[idx];
  };
}

if (typeof strProto.at !== "function") {
  strProto.at = function (this: string, i: number): string | undefined {
    const n = Math.trunc(i) || 0;
    const idx = n < 0 ? this.length + n : n;
    return idx < 0 || idx >= this.length ? undefined : this.charAt(idx);
  };
}

if (typeof arrProto.findLast !== "function") {
  arrProto.findLast = function <T>(
    this: T[],
    pred: (v: T, i: number, a: T[]) => boolean,
  ): T | undefined {
    for (let i = this.length - 1; i >= 0; i -= 1) {
      if (pred(this[i] as T, i, this)) return this[i] as T;
    }
    return undefined;
  };
}

if (typeof arrProto.findLastIndex !== "function") {
  arrProto.findLastIndex = function <T>(
    this: T[],
    pred: (v: T, i: number, a: T[]) => boolean,
  ): number {
    for (let i = this.length - 1; i >= 0; i -= 1) {
      if (pred(this[i] as T, i, this)) return i;
    }
    return -1;
  };
}

type HasOwnStatic = typeof Object & { hasOwn?: (o: object, k: PropertyKey) => boolean };
if (typeof (Object as HasOwnStatic).hasOwn !== "function") {
  (Object as HasOwnStatic).hasOwn = (o: object, k: PropertyKey): boolean =>
    Object.prototype.hasOwnProperty.call(o, k);
}

type StructuredCloneGlobal = typeof globalThis & {
  structuredClone?: <T>(v: T) => T;
};
const scGlobal = globalThis as StructuredCloneGlobal;
if (typeof scGlobal.structuredClone !== "function") {
  /// JSON 克隆回退:预览面用法为纯配置/数据对象(hls.js 等);非 JSON 形
  /// (Error/Map/循环引用)在预览路径无实证,抛错可见可修。
  scGlobal.structuredClone = <T,>(v: T): T => JSON.parse(JSON.stringify(v)) as T;
}
