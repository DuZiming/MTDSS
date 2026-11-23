/// 独立预览窗入口(V4-1 T20 ⧉;field r12 多开重构 2026-11-23):每弹窗
/// 独立窗(popout.rs 标签池:config 预建窗 + 运行时自愈创建)+ eval 注入
/// boot + 页面轮询装配。
///
/// 架构注(field 四轮归因):initialization_script 在 wry/WebView2 有注册
/// 竞态——终形=Rust 侧 `eval`(赋值 boot + 派 `mtdss-boot-set` 事件)+
/// 本页「首次轮询等 boot / 事件重挂」。关窗语义:r12 起 Esc/X=真销毁
/// (每窗独立,无复用;句柄经 CloseRequested 收口;r10 离屏停放随单窗
/// 复用时代退役)。
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
  // field r12(2026-11-23 多开重构):关=真销毁——每弹窗独立窗(popout.rs
  // 标签池自愈创建),无复用无置换;Esc/X 即关。r10 离屏停放(单窗
  // 复用时代的复活通道)随多开退役。
  // 关窗链(field 实测定形):恒 preventDefault 吞原生回合,收口后
  // destroy() 强销毁——close() 会再走 WM_CLOSE→close-requested 事件环,
  // 与本监听器互喂死循环(实测 invoke OK 而窗不亡);destroy 旁路事件。
  const teardownAndDestroy = (): void => {
    teardownCurrent(); // 销毁挂载 + previewClose 释句柄(IPC 已派发,窗亡不碍)
    void win.destroy().catch(() => {});
  };
  // Esc = 关窗。
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") teardownAndDestroy();
  });
  // X 关/Esc 收口面:preventDefault 放 destroy 强销毁(能力=core:window:
  // allow-destroy;标签池全体窗在册)。
  void win.onCloseRequested((event) => {
    event.preventDefault();
    teardownAndDestroy();
  });
}
