import { defineConfig } from "vitest/config";

// 测试面与构建面分置:lib 构建不吃测试配置。
export default defineConfig({
  test: {
    environment: "happy-dom",
    include: ["src/**/*.test.ts"],
  },
});
