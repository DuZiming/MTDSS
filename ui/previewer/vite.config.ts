import { defineConfig } from "vite";

// lib 构建:es 单文件(零 React;pdf/md/text 的重量级依赖按需动态 import,但单文件资产
// 纪律=T16 钉:移动 WebView 拿同一 dist/previewer.js → inlineDynamicImports 全内联,
// 动态 import 语义保留=同文件内惰性求值,不挂载 pdf 不付解析代价)。
// d.ts 由 tsc -p tsconfig.build.json 另出。
//
// r70e target=es2019:旧 Android WebView 实测 Chromium 83(MIX3 实机 dumpsys)——
// vite 缺省 'modules' 档(chrome87 语法面)内含 ??=/||=/&&= 等逻辑赋值(85+)与
// 静态块(94+),83 解析器直接 SyntaxError = 移动端文档族(pdf/文本)整包白屏
// 根因;es2019 档把全部语法降级到 83 可解析。运行时内建(replaceAll/.at 等)
// 语法降级覆盖不了,由 src/compat.ts 垫片兜底;pdf.js 另走 legacy 构建。
export default defineConfig({
  build: {
    target: "es2019",
    lib: {
      entry: "./src/index.ts",
      formats: ["es"],
      fileName: () => "previewer.js",
    },
    rollupOptions: {
      output: {
        inlineDynamicImports: true,
      },
    },
    sourcemap: true,
    outDir: "dist",
    emptyOutDir: true,
  },
});
