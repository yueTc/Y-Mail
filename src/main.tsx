import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import App from "./App";
import ScreenshotOverlay from "./ScreenshotOverlay";
import { applyStoredLanguage, readStoredLanguage, t } from "./i18n";
import { applyStoredTheme } from "./readerTheme";
import "./index.css";
import "./wave7.css";

// 先把存过的深色 / 浅色偏好写到 <html>，界面首帧就是对的。
applyStoredTheme();
// 界面语言同理：先读本地偏好，首帧就是对的。
applyStoredLanguage();
document.documentElement.lang = readStoredLanguage() === "zh" ? "zh-CN" : "en";

const container = document.getElementById("root");
if (!container) {
  throw new Error(t("缺少 #root 挂载点，index.html 可能被改动过。"));
}

/** 截图叠加窗和主窗口共用一份前端产物，靠地址里的参数区分。 */
const isScreenshotWindow = new URLSearchParams(window.location.search).get("window") === "screenshot";

createRoot(container).render(
  <StrictMode>{isScreenshotWindow ? <ScreenshotOverlay /> : <App />}</StrictMode>,
);