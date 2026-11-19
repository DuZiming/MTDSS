import { defineConfig } from "vite";

// lib 构建:es 单文件(零 React、零运行时依赖);d.ts 由 tsc -p tsconfig.build.json 另出。
// 移动端(T22-25 WebView 资产)拿同一 dist/previewer.js。
export default defineConfig({
  build: {
    lib: {
      entry: "./src/index.ts",
      formats: ["es"],
      fileName: () => "previewer.js",
    },
    sourcemap: true,
    outDir: "dist",
    emptyOutDir: true,
  },
});
