import { ensureStyles } from "./styles";
import type { PreviewEvent, PreviewHandle, PreviewRequest } from "./types";

/// 图片查看器完整态机(spec §5.1「缩略图→原文件渐进;渐进中途失败=停在缩略图态+重试钮」):
///
/// 1. boot:thumb 先显(尽力而为,失败静默)→ raw/jpeg 渐进拉取;
/// 2. jpeg(HEIC 转码面)优先于 raw,单请求;
/// 3. raw 成功 → 替换缩略图(revoke 旧字节桥)+ meta{bytes};迟到旧面不得覆回缩略图;
/// 4. raw 失败 → 停缩略图态 + 重试钮 + retry_needed 事件;重试自答(重拉 raw),逐次可重发;
/// 5. zoom/pan:滚轮缩放(1..8 封顶,缩回原大平移归零)+ 鼠标拖拽平移(放大后开门);
///    触摸不接——移动端容器自理(T23/T25 自研手势);
/// 6. destroy:监听摘净、在途请求 abort、字节桥回收、迟火回调按代次弃。

const ZOOM_MIN = 1;
const ZOOM_MAX = 8;
/// 滚轮缩放灵敏度:e^(-deltaY×k),notch(±100/120)约 ×1.16-1.20。
const WHEEL_FACTOR = 0.0015;

type ListenerCleanup = {
  target: EventTarget;
  type: string;
  fn: EventListener;
  options?: AddEventListenerOptions;
};

export function mountImage(
  el: HTMLElement,
  req: PreviewRequest,
  onEvent: (e: PreviewEvent) => void,
): PreviewHandle {
  ensureStyles();

  let destroyed = false;
  /// raw 重试代次:弃旧迟火(重试/destroy 皆翻代)。
  let rawGen = 0;
  /// raw 已显旗:渐进只进不退(缩略图迟到/竞窗不得覆回)。
  let rawShown = false;
  /// 当前在用 objectURL(换面/销毁时回收)。
  let currentUrl: string | null = null;

  // ---- 装配:stage(img)+ 失败覆盖层(note+重试钮) ----
  const root = document.createElement("div");
  root.className = "mtdss-preview mtdss-preview--image";

  const stage = document.createElement("div");
  stage.className = "mtdss-preview__stage";
  const img = document.createElement("img");
  img.className = "mtdss-preview__img";
  img.draggable = false;
  img.alt = req.meta.name;
  stage.appendChild(img);
  root.appendChild(stage);

  const overlay = document.createElement("div");
  overlay.className = "mtdss-preview__overlay mtdss-preview__overlay--hidden";
  const note = document.createElement("div");
  note.className = "mtdss-preview__note";
  note.textContent = "图片加载失败";
  const retryButton = document.createElement("button");
  retryButton.type = "button";
  retryButton.className = "mtdss-preview__button";
  retryButton.textContent = "重试";
  overlay.append(note, retryButton);
  root.appendChild(overlay);
  el.appendChild(root);

  // ---- 监听记账:全走此门,destroy 一次摘净 ----
  const cleanups: ListenerCleanup[] = [];
  const listen = (
    target: EventTarget,
    type: string,
    fn: EventListener,
    options?: AddEventListenerOptions,
  ): ListenerCleanup => {
    target.addEventListener(type, fn, options);
    const entry: ListenerCleanup = { target, type, fn, options };
    cleanups.push(entry);
    return entry;
  };
  const unlisten = (entry: ListenerCleanup): void => {
    entry.target.removeEventListener(entry.type, entry.fn, entry.options);
    const i = cleanups.indexOf(entry);
    if (i >= 0) cleanups.splice(i, 1);
  };

  // ---- zoom/pan(鼠标;触摸留移动端容器自理) ----
  let zoom = ZOOM_MIN;
  let panX = 0;
  let panY = 0;
  const applyTransform = (): void => {
    img.style.transform = `translate(${panX}px, ${panY}px) scale(${zoom})`;
  };
  applyTransform();

  listen(
    stage,
    "wheel",
    ((ev: Event) => {
      const wheel = ev as WheelEvent;
      wheel.preventDefault();
      zoom = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, zoom * Math.exp(-wheel.deltaY * WHEEL_FACTOR)));
      if (zoom <= ZOOM_MIN) {
        // 缩回原大=平移归零(图复原位)
        panX = 0;
        panY = 0;
      }
      applyTransform();
    }) as EventListener,
    { passive: false },
  );

  let dragMove: ListenerCleanup | null = null;
  let dragUp: ListenerCleanup | null = null;
  let lastX = 0;
  let lastY = 0;
  const endDrag = (): void => {
    if (dragMove !== null) unlisten(dragMove);
    dragMove = null;
    if (dragUp !== null) unlisten(dragUp);
    dragUp = null;
    stage.classList.remove("mtdss-preview__stage--dragging");
  };
  listen(stage, "pointerdown", ((ev: Event) => {
    const pointer = ev as PointerEvent;
    // 拖拽门:仅放大后、仅鼠标左键;触摸留给移动端容器
    if (destroyed || pointer.pointerType !== "mouse" || pointer.button !== 0 || zoom <= ZOOM_MIN) return;
    pointer.preventDefault();
    lastX = pointer.clientX;
    lastY = pointer.clientY;
    stage.classList.add("mtdss-preview__stage--dragging");
    dragMove = listen(
      window,
      "pointermove",
      ((ev2: Event) => {
        const move = ev2 as PointerEvent;
        panX += move.clientX - lastX;
        panY += move.clientY - lastY;
        lastX = move.clientX;
        lastY = move.clientY;
        applyTransform();
      }) as EventListener,
    );
    dragUp = listen(window, "pointerup", () => {
      endDrag();
    });
  }) as EventListener);

  // ---- 数据面:fetch → blob → objectURL ----
  let thumbAbort: AbortController | null = null;
  let rawAbort: AbortController | null = null;

  const fetchBlob = async (url: string, signal: AbortSignal): Promise<Blob> => {
    const res = await fetch(url, { signal });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    return res.blob();
  };

  const setSrc = (newBlob: Blob): void => {
    const url = URL.createObjectURL(newBlob);
    if (currentUrl !== null) URL.revokeObjectURL(currentUrl);
    currentUrl = url;
    img.src = url;
    // 换面复位手势:新图从原大原位起
    zoom = ZOOM_MIN;
    panX = 0;
    panY = 0;
    applyTransform();
  };

  const loadThumb = async (): Promise<void> => {
    const url = req.urls.thumb;
    if (url === undefined) return;
    const signal = (thumbAbort = new AbortController()).signal;
    try {
      const thumbBlob = await fetchBlob(url, signal);
      // 尽力而为:已销毁/原图已显(渐进只进不退)即弃
      if (destroyed || rawShown) return;
      setSrc(thumbBlob);
    } catch {
      // 缩略图失败静默落 raw,不与 raw 失败共事件面
    }
  };

  const loadRaw = async (): Promise<void> => {
    // HEIC 转码面优先(T21 消费);退直连 raw
    const url = req.urls.jpeg ?? req.urls.raw;
    if (url === undefined) return;
    const gen = ++rawGen;
    rawAbort?.abort();
    const signal = (rawAbort = new AbortController()).signal;
    try {
      const rawBlob = await fetchBlob(url, signal);
      if (destroyed || gen !== rawGen) return; // 迟火按代次弃
      rawShown = true;
      setSrc(rawBlob);
      overlay.classList.add("mtdss-preview__overlay--hidden");
      onEvent({ type: "meta", bytes: rawBlob.size });
    } catch {
      if (destroyed || gen !== rawGen) return;
      // 停缩略图态(不摘已显面)+ 重试钮 + retry_needed 事件
      overlay.classList.remove("mtdss-preview__overlay--hidden");
      onEvent({ type: "retry_needed", message: "原图加载失败" });
    }
  };

  listen(retryButton, "click", () => {
    void loadRaw();
  });

  // ---- boot:thumb 先显 → raw 渐进 ----
  void loadThumb();
  if ((req.urls.jpeg ?? req.urls.raw) !== undefined) {
    void loadRaw();
  } else {
    // 装配即不可预览:终态 error,无重试面
    note.textContent = "图片不可预览";
    retryButton.remove();
    overlay.classList.remove("mtdss-preview__overlay--hidden");
    onEvent({ type: "error", message: "无可用预览地址" });
  }

  // ---- destroy:监听摘净 + 在途 abort + 字节桥回收 + 迟火弃 ----
  return {
    destroy: () => {
      if (destroyed) return;
      destroyed = true;
      rawGen += 1;
      thumbAbort?.abort();
      rawAbort?.abort();
      endDrag();
      for (const entry of [...cleanups]) unlisten(entry);
      if (currentUrl !== null) URL.revokeObjectURL(currentUrl);
      currentUrl = null;
      root.remove();
    },
  };
}
