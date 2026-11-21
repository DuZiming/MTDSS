import { defineConfig } from "vite";

// lib 构建:es 单文件(零 React;pdf/md/text 的重量级依赖按需动态 import,但单文件资产
// 纪律=T16 钉:移动 WebView 拿同一 dist/previewer.js → inlineDynamicImports 全内联,
// 动态 import 语义保留=同文件内惰性求值,不挂载 pdf 不付解析代价)。
// d.ts 由 tsc -p tsconfig.build.json 另出。
export default defineConfig({
  build: {
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
