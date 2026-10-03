import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 开发约定：端口固定 1420，且不许自动换端口（换了外壳就连不上）。
export default defineConfig({
  plugins: [react()],
  // 清屏会顶掉 Rust 侧日志，开发时保持关闭。
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // 不监听 Rust 产物，避免前端热更新被编译产物触发。
      ignored: ["**/src-tauri/**"],
    },
  },
  // 允许读取 Tauri 注入的环境变量（TAURI_ENV_*）。
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    // 桌面端只跑在 WebView2（Chromium 内核）上，无需兼容老浏览器。
    target: "chrome120",
    sourcemap: true,
  },
});