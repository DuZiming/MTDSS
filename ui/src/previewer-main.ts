/// 独立预览窗入口(V4-1 T20 ⧉):同 previewer bundle(mountPreview 静态
/// import——面板与弹窗同一渲染管线),boot 载荷经 Rust 侧 preview_popout
/// 的 initialization_script 注入(webview 级用户脚本,CSP 不约束、无
/// query 编码面)。窗=多开允许,标题=文件名(Rust 建窗时定)。
///
/// 生命周期:Esc 关窗(mockup 确认点⑥「Esc=关面板,或关独立窗焦点」);
/// 关窗即 previewClose 收口本窗名下句柄引用(onCloseRequested——JS close()
/// 与原生 X 双路都经 close-requested,handler 后未阻止即 destroy,invoke
/// 先行送达;尽力而为,UI 进程崩溃漏关的残项由 daemon 空闲回收兜底,T4
/// 既有语义)。boot 缺席(直接以 previewer.html 起窗/注入失败)→ 防御
/// 文案,不 panic。
import { getCurrentWindow } from "@tauri-apps/api/window";
import { mountPreview, type PreviewRequest } from "@mtdss/previewer";

import { previewClose } from "./lib/daemon";
import { loadTheme } from "./lib/theme";
import "./themes.css";

/// 与主窗同源起步的主题(无闪烁先例同款:落 data-theme 后首帧即用)。
document.documentElement.dataset.theme = loadTheme();

/// preview_popout 注入的 boot 载荷(req=按弹出时凭据装配的 PreviewRequest;
/// handle=该窗名下的句柄引用,窗关即释)。
interface PreviewBoot {
  req: PreviewRequest;
  handle: string;
}

declare global {
  interface Window {
    __MTDSS_PREVIEW_BOOT__?: PreviewBoot;
  }
}

const boot = window.__MTDSS_PREVIEW_BOOT__;
const host = document.getElementById("root");

if (host != null && boot != null) {
  mountPreview(host, boot.req, () => {});
  const win = getCurrentWindow();
  // Esc 关独立窗(win.close() 走 capability core:window:allow-close)。
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") void win.close().catch(() => {});
  });
  // 关窗即释句柄引用(引用计数 -1;面板自己的引用由面板收口)。
  void win.onCloseRequested(() => {
    void previewClose(boot.handle).catch(() => {});
  });
} else if (host != null) {
  host.textContent = "缺少预览参数(请从预览面板弹出)";
}
