import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 在 dev 模式按固定端口加载前端，端口被占则直接失败而不是静默换端口
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    outDir: "dist",
    target: "chrome110",
    emptyOutDir: true,
    rolldownOptions: {
      // 四个 WebView 入口：翻译面板 / 划词图标 / 设置 / 历史记录
      input: {
        main: "index.html",
        icon: "icon.html",
        settings: "settings.html",
        history: "history.html",
      },
    },
  },
});
