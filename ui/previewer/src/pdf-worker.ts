/// pdf.js 真 worker 的内联打包口(r45 用户裁定「一套方案,不搞双轨兜底」):
/// worker 模块原文经 `?raw` 内嵌进单文件 bundle(移动 WebView 拿同一
/// dist/previewer.js,零旁挂资产文件——pptx-worker 同纪律),以 blob URL
/// 交给 pdf.js(`GlobalWorkerOptions.workerSrc`)——pdf.js v6 自行以
/// `{type:"module"}` 创建/终结每文档 worker,生命周期归它,本侧零管理。
///
/// 为什么取代主线程假工(r24 决策的撤销):假工把 xref 解析/对象图/
/// 首页渲染全压主线程——大 PDF(60MB 级)即 UI 冻结白屏,渲染永不
/// 完成(用户实机;Chrome 原生 PDFium 不犯是引擎差异)。真 worker 五端
/// 通用:blob URL 的模块 worker 是 WebView2/WebKit(iOS 26)/Android
/// WebView 的共同能力,pptx-worker 已在实机验证 blob worker 面
/// (CSP worker-src blob: 契约既有)。
///
/// 单 scheme 纪律:无平台分支、无假工回退——worker 起不来即 bug,
/// 修本模块,不静默换路(排查单点,用户裁定 2026-11-26)。
import PdfWorkerSource from "pdfjs-dist/build/pdf.worker.min.mjs?raw";

/// blob URL 惰性单例(不挂 PDF 不付创建代价;URL 永存——worker 由
/// pdf.js 按文档创建,重复传同一 src 零成本)。
let cachedUrl: string | null = null;

export function pdfWorkerSrc(): string {
  if (cachedUrl === null) {
    cachedUrl = URL.createObjectURL(
      new Blob([PdfWorkerSource], { type: "text/javascript" }),
    );
  }
  return cachedUrl;
}
