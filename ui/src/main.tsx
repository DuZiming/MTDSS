import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./i18n";
import "./themes.css";
import { loadTheme } from "./lib/theme";

// 无闪烁启动(P4b Task 1,设计 §4):createRoot 渲染前同步落 data-theme,
// 首帧即用持久化主题(CSP 禁内联脚本,非默认主题冷启动残余一帧可接受)。
document.documentElement.dataset.theme = loadTheme();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
