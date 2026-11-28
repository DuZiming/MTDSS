import { defineConfig } from "vite";

// r70e:pdf.js worker 预降级构建(Chromium 83 实测面)。
//
// pdf-worker.ts 以 `?raw` 原文内嵌 worker——不经 vite/oxc 语法降级;
// pdfjs-dist 6.x 即使 legacy 构建仍含 class static 块(ES2022/Chrome 94+)
// 与逻辑赋值 ??=(ES2021/Chrome 85+),旧 Android WebView(MIX3 实机
// dumpsys = com.google.android.webview 83.0.4103.101)解析器直接
// SyntaxError = pdf 预览不可用根因。本配置把 legacy worker 单独过一遍
// es2019 档产 src/pdf-worker-vendor.mjs(static 块/逻辑赋值/#私有字段
// 全降级),npm run build 先跑本步再主构建。
//
// **outDir=src + emptyOutDir:false**——绝不可开 empty(会把整个 src/ 清空)。
export default defineConfig({
  build: {
    target: "es2019",
    lib: {
      entry: "../node_modules/pdfjs-dist/legacy/build/pdf.worker.min.mjs",
      formats: ["es"],
      fileName: () => "pdf-worker-vendor.mjs",
    },
    rollupOptions: {
      output: { inlineDynamicImports: true },
    },
    outDir: "src",
    emptyOutDir: false,
    sourcemap: false,
  },
});
