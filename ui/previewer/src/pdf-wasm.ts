/// pdf.js v6 wasm 解码器基址(r47:JBig2/JPX 扫描件页根修)。
///
/// pdf.js 6.x 的图像解码器(JBig2 扫描件/openjpeg JPX/qcms ICC)是 wasm
/// 伴随资产,worker 需按 `wasmUrl + 文件名` 取之。blob worker 起时此路
/// 断(相对解析到 null = `Ensure that the wasmUrl API parameter is
/// provided` / `nulljbig2_nowasm_fallback.js` → `JBig2 failed to
/// initialize` → 扫描件页全空白,首页非 JBIG2 故独活)。
///
/// 修法(单 scheme,五端同径):**主线程取数**——getDocument 携
/// `useWorkerFetch: false`,worker 经 FetchBinaryData 请主线程 fetch
/// `wasmUrl + 文件名`(页面上下文同源取,零 CORS 面),字节经消息进
/// worker 实例化。wasmUrl = bundle 同目录的 `wasm/` 前缀——各端该
/// 目录都是 previewer.js 的伴生资产:
/// - 桌面:dist/wasm/(Tauri 静态同源);
/// - iOS:资产臂扩展 /wasm/* 预览(App bundle previewer/wasm/);
/// - Android:assets 同构(后续包波)。
/// wasm 资产基址:`<bundle 同目录>/wasm/`。bundle 单文件被各端以真实
/// URL 装载(import.meta.url = previewer.js 的 URL),相对解析稳定。
/// 测试环境(vitest,jsdom)import.meta.url 可能非 http——回退相对
/// 前缀(测试 pdfjs 全 vi.mock,不触真取数)。
export function pdfWasmUrl(): string {
  try {
    return new URL("wasm/", import.meta.url).href;
  } catch {
    return "wasm/";
  }
}

/// 是否随 getDocument 携 wasm 面(主线程取数形)。保留独立出口供测试
/// 注入覆盖。
export const pdfWasmParams = (): { wasmUrl: string; useWorkerFetch: boolean } => ({
  wasmUrl: pdfWasmUrl(),
  useWorkerFetch: false,
});
