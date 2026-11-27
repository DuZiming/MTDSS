/// 音频查看器(HTML5 `<audio>` 直连 urls.raw,V4-1 T21;controls 原生面):
///
/// 1. boot:src=urls.raw(查询令牌 ?t= 直连,CSP media-src 回环面 T20 已
///    放行);loadedmetadata → meta 事件(主内容就绪,bytes=meta.size——
///    面板信息条时长由 audio 元素现取恒可得);
/// 2. 错误面(image 房法):audio error → 停前态 + 覆盖层失败词 + 重试钮
///    + retry_needed 事件;重试 = audio.load() 自答重拉,逐次可重发;
/// 3. urls.raw 缺 = 装配即不可预览:终态 error,无重试面;
/// 4. destroy:pause + src 摘 + load()(浏览器释放字节面)+ 监听摘净。
///
/// 文案全部取自容器缝 strings(T16-M1;缺省中文由 router 解析注入)。

import type { PreviewStrings } from "./strings";
import type { PreviewEvent, PreviewHandle, PreviewRequest } from "./types";

type ListenerCleanup = {
  target: EventTarget;
  type: string;
  fn: EventListener;
};

/// 秒 → mm:ss(≥1h 才显小时位;未知/非有限 = "--:--")。
function fmtDuration(sec: number): string {
  if (!(sec > 0) || !Number.isFinite(sec)) return "--:--";
  const s = Math.floor(sec % 60);
  const m = Math.floor((sec / 60) % 60);
  const h = Math.floor(sec / 3600);
  const two = (n: number): string => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${two(m)}:${two(s)}` : `${two(m)}:${two(s)}`;
}

export function mountAudio(
  el: HTMLElement,
  req: PreviewRequest,
  onEvent: (e: PreviewEvent) => void,
  strings: PreviewStrings,
): PreviewHandle {
  let destroyed = false;

  // ---- 装配:stage(音频卡)+ 失败覆盖层(note+重试钮) ----
  const root = document.createElement("div");
  root.className = "mtdss-preview mtdss-preview--audio";

  const stage = document.createElement("div");
  stage.className =
    "mtdss-preview__stage mtdss-preview__stage--media mtdss-preview__stage--audio";

  // r53 音频卡:图标/文件名/时长/控制条(黑底居中列——旧面浅底一根细
  // 控制条在弹窗里视觉近似空白 = 用户「白屏」真因;视频有画面撑满,
  // 音频以卡面立存在感,黑底与视频观看面一致)
  const card = document.createElement("div");
  card.className = "mtdss-preview__audiocard";
  const icon = document.createElement("div");
  icon.className = "mtdss-preview__audio-icon";
  icon.innerHTML =
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>';
  const nameLine = document.createElement("div");
  nameLine.className = "mtdss-preview__audio-name";
  nameLine.textContent = req.meta.name;
  nameLine.title = req.meta.name;
  const durLine = document.createElement("div");
  durLine.className = "mtdss-preview__audio-dur";
  durLine.textContent = "--:--";
  const audio = document.createElement("audio");
  audio.className = "mtdss-preview__media";
  audio.controls = true;
  audio.preload = "metadata";
  card.append(icon, nameLine, durLine, audio);
  stage.appendChild(card);
  root.appendChild(stage);

  const overlay = document.createElement("div");
  overlay.className = "mtdss-preview__overlay mtdss-preview__overlay--hidden";
  const note = document.createElement("div");
  note.className = "mtdss-preview__note";
  note.textContent = strings.audioLoadFailed;
  const retryButton = document.createElement("button");
  retryButton.type = "button";
  retryButton.className = "mtdss-preview__button";
  retryButton.textContent = strings.retry;
  overlay.append(note, retryButton);
  root.appendChild(overlay);
  el.appendChild(root);

  // ---- 监听记账:全走此门,destroy 一次摘净 ----
  const cleanups: ListenerCleanup[] = [];
  const listen = (target: EventTarget, type: string, fn: EventListener): void => {
    target.addEventListener(type, fn);
    cleanups.push({ target, type, fn });
  };

  const showError = (): void => {
    overlay.classList.remove("mtdss-preview__overlay--hidden");
    onEvent({ type: "retry_needed", message: strings.audioRetryNeeded });
  };

  listen(audio, "error", showError);
  listen(audio, "loadedmetadata", () => {
    if (destroyed) return;
    durLine.textContent = fmtDuration(audio.duration);
    overlay.classList.add("mtdss-preview__overlay--hidden");
    onEvent({ type: "meta", bytes: req.meta.size });
  });

  listen(retryButton, "click", () => {
    if (destroyed) return;
    overlay.classList.add("mtdss-preview__overlay--hidden");
    audio.load(); // 重拉现 src(重试自答,逐次可重发)
  });

  // ---- boot:src 直连;缺 raw = 终态 error ----
  if (req.urls.raw === undefined) {
    audio.remove();
    retryButton.remove();
    note.textContent = strings.unavailableMessage;
    overlay.classList.remove("mtdss-preview__overlay--hidden");
    onEvent({ type: "error", message: strings.unavailableMessage });
  } else {
    audio.src = req.urls.raw;
  }

  // ---- destroy:监听摘净 + 媒体面释放 + 装配 DOM 摘 ----
  return {
    destroy: () => {
      if (destroyed) return;
      destroyed = true;
      audio.pause();
      audio.removeAttribute("src");
      audio.load(); // 空 src load = 浏览器释放字节面
      for (const entry of [...cleanups]) {
        entry.target.removeEventListener(entry.type, entry.fn);
      }
      root.remove();
    },
  };
}
