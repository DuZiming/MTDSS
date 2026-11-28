import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./i18n";
import "./themes.css";
// 预览包静态样式(field r14 #2 修):右侧栏封面(mountCover)的
// .mtdss-cover* 族在此窗生效——缺此 import 封面元素零样式(靠上溢出/
// 与元数据叠字);弹窗页(previewer-main)各自引入同源。
import "@mtdss/previewer/styles.css";
import { loadTheme } from "./lib/theme";

// 无闪烁启动(P4b Task 1,设计 §4):createRoot 渲染前同步落 data-theme,
// 首帧即用持久化主题(CSP 禁内联脚本,非默认主题冷启动残余一帧可接受)。
document.documentElement.dataset.theme = loadTheme();

// field 诊断(2026-11-22 二开⧉无反应归因轮):全局错误钩子——任何未捕获
// 异常/拒绝立刻 visible(title 前缀 + 屏幕留痕 div,toast 系统可能随之死)。
// title 最硬:任务栏/窗标题一秒可见。
const diagErr = (label: string, e: unknown): void => {
  const msg = e instanceof Error ? `${e.message}` : String(e);
  document.title = `!!JS ${label}: ${msg}`.slice(0, 120);
  const div = document.createElement("div");
  div.textContent = `!!JS ${label}: ${msg}`;
  div.style.cssText =
    "position:fixed;top:0;left:0;z-index:2147483647;background:#c00;color:#fff;padding:4px 8px;font:12px monospace;max-width:100vw;word-break:break-all";
  document.body.appendChild(div);
};
window.addEventListener("error", (e) => diagErr("err", e.error ?? e.message));
window.addEventListener("unhandledrejection", (e) => diagErr("rej", e.reason));

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
