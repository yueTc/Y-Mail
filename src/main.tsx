import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import App from "./App";
import "./index.css";
import "./wave7.css";

const container = document.getElementById("root");
if (!container) {
  throw new Error("缺少 #root 挂载点，index.html 可能被改动过。");
}

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);