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

import { ensureStyles } from "./styles";
import type { PreviewStrings } from "./strings";
import type { PreviewEvent, PreviewHandle, PreviewRequest } from "./types";

type ListenerCleanup = {
  target: EventTarget;
  type: string;
  fn: EventListener;
};

export function mountAudio(
  el: HTMLElement,
  req: PreviewRequest,
  onEvent: (e: PreviewEvent) => void,
  strings: PreviewStrings,
): PreviewHandle {
  ensureStyles();

  let destroyed = false;

  // ---- 装配:stage(audio)+ 失败覆盖层(note+重试钮) ----
  const root = document.createElement("div");
  root.className = "mtdss-preview mtdss-preview--audio";

  const stage = document.createElement("div");
  stage.className = "mtdss-preview__stage mtdss-preview__stage--media";
  const audio = document.createElement("audio");
  audio.className = "mtdss-preview__media";
  audio.controls = true;
  audio.preload = "metadata";
  stage.appendChild(audio);
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
