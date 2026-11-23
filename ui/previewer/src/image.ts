import type { PreviewStrings } from "./strings";
import type { PreviewEvent, PreviewHandle, PreviewRequest } from "./types";

/// 图片查看器完整态机(spec §5.1「缩略图→原文件渐进;渐进中途失败=停在缩略图态+重试钮」):
///
/// 1. boot:thumb 先显(尽力而为,失败静默)→ raw/jpeg 渐进拉取;
/// 2. jpeg(HEIC 转码面)优先于 raw,单请求;
/// 3. raw 成功 → 替换缩略图(revoke 旧字节桥)+ meta{bytes};迟到旧面不得覆回缩略图;
/// 4. raw 失败 → 停缩略图态 + 重试钮 + retry_needed 事件;重试自答(重拉 raw),逐次可重发;
/// 5. zoom/pan:滚轮缩放(1..8 封顶,缩回 fit 底线平移归零)+ 鼠标拖拽
///    平移(放大后开门);起步=加载后的原大 100%(field r13 #1,fit 经
///    双击/滚轮下探;移动端容器不接——T23/T25 自研手势);
/// 6. destroy:监听摘净、在途请求 abort、字节桥回收、迟火回调按代次弃。
///
/// 文案全部取自容器缝 strings(T16-M1;缺省中文由 router 解析注入)。

const ZOOM_MAX = 8;
/// 绝对下限(允许缩小于 fit;回到 fit 即平移归零)。
const ZOOM_MIN_ABS = 0.1;
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
  strings: PreviewStrings,
  nav?: { prev(): void; next(): void },
): PreviewHandle {

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

  /// 小地图/控制条元素(装配后填充)
  let minimapBox: HTMLDivElement | null = null;
  let minimapImg: HTMLImageElement | null = null;
  let minimapView: HTMLDivElement | null = null;
  let zoomLabel: HTMLDivElement | null = null;

  // ---- 小地图(左下;field 定稿 2026-11-23) ----
  const minimap = document.createElement("div");
  minimap.className = "mtdss-preview__minimap";
  const minimapInner = document.createElement("div");
  minimapInner.className = "mtdss-preview__minimap-inner";
  minimapBox = minimapInner;
  minimapImg = document.createElement("img");
  minimapImg.draggable = false;
  minimapImg.alt = "";
  minimapView = document.createElement("div");
  minimapView.className = "mtdss-preview__minimap-view";
  minimapInner.append(minimapImg, minimapView);
  minimap.appendChild(minimapInner);
  root.appendChild(minimap);

  // ---- 控制条(底部居中:上一张/缩小/倍率/放大/下一张) ----
  const controls = document.createElement("div");
  controls.className = "mtdss-preview__img-controls";
  const mkBtn = (label: string, onClick: () => void): HTMLButtonElement => {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "mtdss-preview__img-ctl";
    b.textContent = label;
    b.addEventListener("click", onClick);
    return b;
  };
  const prevBtn = nav === undefined ? null : mkBtn("‹", () => onEvent({ type: "nav", dir: "prev" }));
  const zoomOutBtn = mkBtn("−", () => zoomBy(1 / 1.25));
  zoomLabel = document.createElement("div");
  zoomLabel.className = "mtdss-preview__img-zoom-label";
  zoomLabel.textContent = "100%";
  const zoomInBtn = mkBtn("+", () => zoomBy(1.25));
  // 恢复钮(field r12 定稿 2026-11-23,「适配」文字钮退役):内联缓存
  // SVG 图标(恢复/重置旋转箭头),点击=倍率回 100% 原大+平移归零。fit
  // 起步语义保留在装配面与双击切换;图标零外联(CSP 同族纪律)。
  const RESTORE_SVG =
    '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true">' +
    '<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>' +
    '<path d="M3 3v5h5" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>' +
    "</svg>";
  const resetBtn = document.createElement("button");
  resetBtn.type = "button";
  resetBtn.className = "mtdss-preview__img-ctl";
  resetBtn.title = "恢复 100%";
  resetBtn.setAttribute("aria-label", "恢复 100%");
  resetBtn.innerHTML = RESTORE_SVG;
  resetBtn.addEventListener("click", () => {
    zoom = 1;
    panX = 0;
    panY = 0;
    applyTransform();
  });
  const nextBtn = nav === undefined ? null : mkBtn("›", () => onEvent({ type: "nav", dir: "next" }));
  if (prevBtn !== null) controls.appendChild(prevBtn);
  controls.append(zoomOutBtn, zoomLabel, zoomInBtn, resetBtn);
  if (nextBtn !== null) controls.appendChild(nextBtn);
  root.appendChild(controls);

  const overlay = document.createElement("div");
  overlay.className = "mtdss-preview__overlay mtdss-preview__overlay--hidden";
  const note = document.createElement("div");
  note.className = "mtdss-preview__note";
  note.textContent = strings.imageLoadFailed;
  const retryButton = document.createElement("button");
  retryButton.type = "button";
  retryButton.className = "mtdss-preview__button";
  retryButton.textContent = strings.retry;
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

  // ---- zoom/pan(field r13 #1:fit 基准 + 原大 100% 起步) ----
  /// fitScale = 当前图在容器内的完整可见比例(≤1);zoom 相对自然尺寸。
  /// 起步 = 加载后的原大 100%(field r13 用户裁定);显示百分比 =
  /// zoom×100%。fit=双击切换/滚轮下探底线。
  let fitScale = 1;
  let zoom = 1;
  let panX = 0;
  let panY = 0;
  let naturalW = 0;
  let naturalH = 0;
  const computeFit = (): number => {
    if (naturalW === 0 || naturalH === 0) return 1;
    const cw = stage.clientWidth;
    const ch = stage.clientHeight;
    if (cw === 0 || ch === 0) return 1;
    return Math.min(1, cw / naturalW, ch / naturalH);
  };
  const clampPan = (): void => {
    // 平移限幅:图不拖出容器(留 8px 余量);容器未布局(0 尺,测试/
    // 装配前帧)不限幅——否则全 pan 被 0 上限清零。
    const cw = stage.clientWidth;
    const ch = stage.clientHeight;
    if (cw === 0 || ch === 0) return;
    const dw = naturalW * zoom - cw;
    const dh = naturalH * zoom - ch;
    const maxX = dw > 0 ? dw / 2 + 8 : 0;
    const maxY = dh > 0 ? dh / 2 + 8 : 0;
    panX = Math.min(maxX, Math.max(-maxX, panX));
    panY = Math.min(maxY, Math.max(-maxY, panY));
  };
  const applyTransform = (): void => {
    clampPan();
    img.style.transform = `translate(${panX}px, ${panY}px) scale(${zoom})`;
    if (zoomLabel !== null) zoomLabel.textContent = `${Math.round(zoom * 100)}%`;
    // 小地图视口框:可见区(容器)映射到 minimap 坐标系
    if (minimapView !== null && minimapBox !== null && naturalW > 0) {
      const cw = stage.clientWidth;
      const ch = stage.clientHeight;
      const boxW = minimapBox.clientWidth;
      const boxH = minimapBox.clientHeight;
      // minimap 图铺满盒(等比 letterbox,取主轴比例)
      const r = Math.min(boxW / naturalW, boxH / naturalH);
      const mapW = naturalW * r;
      const mapH = naturalH * r;
      // 图中心在容器中的偏移 = pan;可见半宽 = cw/2
      const cx = naturalW / 2 - panX / zoom;
      const cy = naturalH / 2 - panY / zoom;
      const vw = cw / zoom;
      const vh = ch / zoom;
      // 视口矩形钳到图面(field r12 修 #1):fit 态视口大于图(整图可见
      // + 容器留白),裸映射矩形会越出小地图图面——初始蓝框飞到图外不居
      // 中。钳到交集后,整图可见 ⇒ 框=全图面(恒居中);部分可见=真实
      // 可见区。矩形面积恒 ≥0。
      const ix0 = (boxW - mapW) / 2;
      const iy0 = (boxH - mapH) / 2;
      let vx0 = (cx - vw / 2) * r + ix0;
      let vy0 = (cy - vh / 2) * r + iy0;
      const vx1 = Math.min(vx0 + vw * r, ix0 + mapW);
      const vy1 = Math.min(vy0 + vh * r, iy0 + mapH);
      vx0 = Math.max(vx0, ix0);
      vy0 = Math.max(vy0, iy0);
      minimapView.style.left = `${Math.max(0, vx0).toFixed(1)}px`;
      minimapView.style.top = `${Math.max(0, vy0).toFixed(1)}px`;
      minimapView.style.width = `${Math.max(0, vx1 - vx0).toFixed(1)}px`;
      minimapView.style.height = `${Math.max(0, vy1 - vy0).toFixed(1)}px`;
    }
  };
  const zoomBy = (factor: number): void => {
    zoom = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN_ABS, zoom * factor));
    if (zoom <= fitScale) {
      zoom = fitScale;
      panX = 0;
      panY = 0;
    }
    applyTransform();
  };
  const fitView = (): void => {
    fitScale = computeFit();
    zoom = fitScale;
    panX = 0;
    panY = 0;
    applyTransform();
  };

  applyTransform(); // 初始形(无图也落地 translate(0,0) scale(1))

  listen(
    stage,
    "wheel",
    ((ev: Event) => {
      const wheel = ev as WheelEvent;
      wheel.preventDefault();
      zoomBy(Math.exp(-wheel.deltaY * WHEEL_FACTOR));
    }) as EventListener,
    { passive: false },
  );
  // 双击 = fit 与 100% 切换(图片查看器惯例)
  listen(
    stage,
    "dblclick",
    (() => {
      if (zoom > fitScale) fitView();
      else {
        zoom = 1;
        applyTransform();
      }
    }) as EventListener,
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
    // 拖拽门(field 定稿):中键恒可拖(平移);左键仅放大后;触摸留移动端容器
    const middle = pointer.button === 1;
    const leftZoomed = pointer.button === 0 && zoom > fitScale;
    if (destroyed || pointer.pointerType !== "mouse" || !(middle || leftZoomed)) return;
    if (middle) ev.preventDefault?.(); // 抑制中键自动滚动
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
    if (minimapImg !== null) minimapImg.src = url;
    // 自然尺寸就绪 → 以加载后的图为基准重置(field r13 #1:初始=原大
    // 100%,平移归零——替换面(thumb→raw)各归各重置,倍率标签恒以
    // 现图原大为基准,不再闪「先 100% 后适配」的过程;fit 经双击/滚轮
    // 缩到 fitScale 底线取)。
    const onLoad = (): void => {
      naturalW = img.naturalWidth;
      naturalH = img.naturalHeight;
      fitScale = computeFit();
      zoom = 1;
      panX = 0;
      panY = 0;
      applyTransform();
    };
    if (img.complete && img.naturalWidth > 0) onLoad();
    else listen(img, "load", onLoad as EventListener);
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
      minimap.classList.remove("mtdss-preview__minimap--hidden");
      controls.classList.remove("mtdss-preview__img-controls--hidden");
      setSrc(rawBlob);
      overlay.classList.add("mtdss-preview__overlay--hidden");
      onEvent({ type: "meta", bytes: rawBlob.size });
    } catch {
      if (destroyed || gen !== rawGen) return;
      // 停缩略图态(不摘已显面)+ 重试钮 + retry_needed 事件
      overlay.classList.remove("mtdss-preview__overlay--hidden");
      minimap.classList.add("mtdss-preview__minimap--hidden");
      controls.classList.add("mtdss-preview__img-controls--hidden");
      onEvent({ type: "retry_needed", message: strings.imageRetryNeeded });
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
    note.textContent = strings.imageUnavailable;
    retryButton.remove();
    overlay.classList.remove("mtdss-preview__overlay--hidden");
    onEvent({ type: "error", message: strings.unavailableMessage });
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
