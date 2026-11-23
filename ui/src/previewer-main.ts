/// 独立预览窗入口(V4-1 T20 ⧉;field 重架构 2026-11-22):预建隐藏窗
/// (tauri.conf `preview-popout`)复用 + eval 注入 + 事件驱动重挂载。
///
/// 架构注(field 三轮归因):initialization_script 在 wry/WebView2 有注册
/// 竞态、运行时建窗在本机 webview 永不渲染——终形=config 预建窗 +
/// Rust 侧 `eval`(赋值 boot + 派 `mtdss-boot-set` 事件)+ 本页
/// 「首次轮询等 boot / 后续事件重挂」。单窗复用:新弹出=销毁旧预览
/// +释旧句柄+挂新内容;Esc/X=hide(不 destroy)+收口句柄。
import { PhysicalPosition } from "@tauri-apps/api/dpi";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { mountPreview, type PreviewRequest } from "@mtdss/previewer";
// 静态样式(2026-11-23 CSP 根因修):弹窗页样式必须走构建期 css <link>
// (同源放行);运行时 <style> 注入会被服务面 CSP 拒(弹窗未样式化根因)。
import "@mtdss/previewer/styles.css";

import { previewClose } from "./lib/daemon";
import { loadTheme } from "./lib/theme";
import "./themes.css";

/// 与主窗同源起步的主题(无闪烁先例同款:落 data-theme 后首帧即用)。
document.documentElement.dataset.theme = loadTheme();

/// preview_popout 注入的 boot 载荷(req=按弹出时凭据装配的 PreviewRequest;
/// handle=该窗名下的句柄引用,收口即释)。
interface PreviewBoot {
  req: PreviewRequest;
  handle: string;
}

declare global {
  interface Window {
    __MTDSS_PREVIEW_BOOT__?: PreviewBoot;
  }
}

const host = document.getElementById("root");
const BOOT_POLL_MS = 50;

/// 当前挂载(单窗复用的换面锚:destroy + 句柄释放)。
let current: { destroy(): void; handle: string } | null = null;

const teardownCurrent = (): void => {
  if (current === null) return;
  current.destroy();
  void previewClose(current.handle).catch(() => {});
  current = null;
};

/// 换面(首次与 mtdss-boot-set 事件共用):销毁旧 → 挂新 → 标题=文件名。
/// 幂等门:同一载荷(事件与首次轮询双触发)只挂一次。
let lastAppliedHandle: string | null = null;
const applyBoot = (): void => {
  const b = window.__MTDSS_PREVIEW_BOOT__;
  if (host === null || b === null || b === undefined) return;
  if (b.handle === lastAppliedHandle) return;
  lastAppliedHandle = b.handle;
  teardownCurrent();
  const mounted = mountPreview(host, b.req, () => {});
  current = { destroy: mounted.destroy, handle: b.handle };
  document.title = b.req.meta.name;
};

/// 监听器可卸载缝(happy-dom 测试环境 window 跨用例复用,上一实例的
/// 监听器会双触发;生产单次加载无重入,此缝零成本)。
const bootListenerAbort = new AbortController();
window.addEventListener("mtdss-boot-set", applyBoot, {
  signal: bootListenerAbort.signal,
});
(window as unknown as { __MTDSS_PREVIEW_TEARDOWN__?: () => void }).__MTDSS_PREVIEW_TEARDOWN__ =
  () => bootListenerAbort.abort();

/// 首次等待(预建隐藏窗 boot 可迟到任意久——永续轮询;boot 在场即挂)。
const boot = await new Promise<PreviewBoot | null>((resolve) => {
  const tick = (): void => {
    const b = window.__MTDSS_PREVIEW_BOOT__;
    if (b !== undefined) resolve(b);
    else setTimeout(tick, BOOT_POLL_MS);
  };
  tick();
});

if (host != null && boot != null) {
  applyBoot();
  const win = getCurrentWindow();
  // field 终形(2026-11-22 二开失败):关 = 离屏停放(不 hide 不 destroy
  // ——hide 后 show/eval 在 WebView2 全灭,destroy 后无法复活;离屏窗恒
  // "visible",注入与复活零障碍)。销毁挂载 + 释句柄照旧。
  const offscreenAndTeardown = (): Promise<void> => {
    teardownCurrent();
    return win.setPosition(new PhysicalPosition(-32000, -32000));
  };
  // Esc = 离屏停放(mockup 确认点⑥)。
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") void offscreenAndTeardown().catch(() => {});
  });
  // X 关 = 阻止 destroy + 离屏停放 + 收口句柄(面板自己的引用由面板收口)。
  void win.onCloseRequested((event) => {
    event.preventDefault();
    void offscreenAndTeardown().catch(() => {});
  });
}
