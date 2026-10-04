import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Tauri 开发约定：端口固定，不许自动换端口（换了外壳就连不上）。
// 1420 落在 Windows 的保留端口段（1374-1473，Hyper-V/WSL 动态保留）里会被拒绝绑定，
// 所以改用 5173（Vite 默认端口，在保留段之外）。
export default defineConfig({
  plugins: [react()],
  // 清屏会顶掉 Rust 侧日志，开发时保持关闭。
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    watch: {
      // 不监听 Rust 产物，避免前端热更新被编译产物触发。
      ignored: ["**/src-tauri/**"],
    },
  },
  // 允许读取 Tauri 注入的环境变量（TAURI_ENV_*）。
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  // 前端自动化测试：跑在 jsdom 里，测试文件与被测代码放在一起。
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}"],
  },
  build: {
    // 桌面端只跑在 WebView2（Chromium 内核）上，无需兼容老浏览器。
    target: "chrome120",
    sourcemap: true,
  },
});