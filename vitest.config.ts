import { defineConfig } from "vitest/config";

// 只为纯逻辑单测使用：不加载 vite.config.ts 的多页面 build 配置，
// 也不起浏览器环境（被测函数全是纯函数）。
export default defineConfig({
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});
