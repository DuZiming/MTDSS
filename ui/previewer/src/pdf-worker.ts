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
/// r70e:**预降级产物**(现代 Android WebView 实测 Chromium 83——pdfjs
/// 6.x 即使 legacy 构建仍含 static 块(94+)/??=(85+),83 解析器拒载)。
/// `?raw` 原文不经 esbuild 降级,故 build 先经 scripts/build-worker83.mjs
/// 预变换出本文件(es2019 档:static 块/逻辑赋值/#私有字段全降级)。
/// worker 内建方法面(replaceAll/.at/Object.hasOwn)由下方 PRELUDE 垫。
import PdfWorkerSource from "./pdf-worker-vendor.mjs?raw";

/// worker 内建垫序(Chromium 83 面;worker 独立全局,compat.ts 主线程垫
/// 不穿透)——只垫 legacy worker 实测用到的那三枚。replaceAll 取字面
/// split/join 形(worker 内两处用法均为字面量替换,零 $-模式面)。
const PRELUDE = `(function(){
if(!String.prototype.replaceAll){Object.defineProperty(String.prototype,"replaceAll",{value:function(s,r){return this.split(s).join(r)},writable:true,configurable:true});}
if(!Array.prototype.at){Object.defineProperty(Array.prototype,"at",{value:function(i){i=Math.trunc(i)||0;if(i<0)i+=this.length;return i<0||i>=this.length?undefined:this[i]},writable:true,configurable:true});}
if(!Object.hasOwn){Object.hasOwn=function(o,k){return Object.prototype.hasOwnProperty.call(o,k)};}
})();`;

/// blob URL 惰性单例(不挂 PDF 不付创建代价;URL 永存——worker 由
/// pdf.js 按文档创建,重复传同一 src 零成本)。
let cachedUrl: string | null = null;

export function pdfWorkerSrc(): string {
  if (cachedUrl === null) {
    cachedUrl = URL.createObjectURL(
      new Blob([PRELUDE + "\n" + PdfWorkerSource], { type: "text/javascript" }),
    );
  }
  return cachedUrl;
}
