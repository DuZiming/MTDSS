/// 第三方无类型子路径的环境声明(vendor seams,集中一处):
/// - pdf.js worker 模块仅取副效果(置 globalThis.pdfjsWorker → 主线程假工),无类型面;
/// - highlight.js 子路径(core/逐语言模块)无随包 d.ts,以环境声明供形;
/// - Vite `?raw` 查询串模块(@file-viewer/pptx 解析 worker 原文内嵌口,见 pptx-worker.ts):
///   默认导出文件原文字符串。

declare module "pdfjs-dist/build/pdf.worker.min.mjs";

/// r70e legacy 主构建(类型面沿用包根声明;Chromium 83 实测注记见 pdf.ts)。
declare module "pdfjs-dist/legacy/build/pdf.mjs" {
  export * from "pdfjs-dist";
}

declare module "highlight.js/lib/core" {
  const hljs: import("highlight.js").HLJSApi;
  export default hljs;
}

declare module "highlight.js/lib/languages/*" {
  const language: import("highlight.js").LanguageFn;
  export default language;
}

declare module "*?raw" {
  const source: string;
  export default source;
}
declare module "*.css";
