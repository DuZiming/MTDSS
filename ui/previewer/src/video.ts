/// 视频查看器(hls.js 挂 urls.hls,V4-1 T21;spec §5.1 降级链):
///
/// 1. boot:海报帧先显(urls.thumb → video.poster 直连,CSP img-src 回环面
///    T20 已放行)→ playlist 预检(M7,见下)→ hls.js 装载(Hls.isSupported()
///    门;主线程 MSE 面,WebView2/移动 WebView 全成立);
/// 2. **T15-M7 零段轮询**:daemon 首刷 = init-only 瞬态零段 playlist(纯头
///    无 EXTINF)——预检 fetch 见纯头即按 TARGETDURATION 轮询重载,直到首段
///    出现或超时(VIDEO_POLL_TIMEOUT_MS);playlist 请求失败(503 面)同环
///    退避重试;超时 → 失败面(引导+重试钮)+ retry_needed;
/// 3. **503 seg_retry 消费(T15/T15b)**:FRAG_LOAD_ERROR/FRAG_LOAD_TIMEOUT
///    且 networkDetails.status=503 → 读 Retry-After(segBackoffDelayMs 纯
///    函数:Retry-After 对齐节律/缺席指数退避,双路封顶 ~10s)→ stopLoad +
///    spinner(videoPreparing)→ 延时 startLoad;FRAG_LOADED 清退避账;
///    503 携 `source_offline` 稳定词或退避次数烧尽 → 失败面(源不再自愈面,
///    重试环退出);
/// 4. hls 致命态/不支持门 → 失败面 = 海报帧打底 + 失败词 + 引导文案(spec
///    §5.1:桌面装 HEVC 扩展/手机改设/下载打开)+ 重试钮(装扩展后不重开
///    窗即可再试)+ 下载链接(urls.raw);
/// 5. meta 事件:MANIFEST_PARSED(主内容就绪;面板信息条时长由 video 元素
///    现取恒可得,不占事件面);
/// 6. destroy:在途定时器摘、hls.destroy、监听摘净、迟火按代次弃。
///
/// 文案全部取自容器缝 strings(T16-M1;缺省中文由 router 解析注入)。
/// 时序缝 timers 可注入(测试手控;真面 setTimeout)。

import { segBackoffDelayMs } from "./backoff";
import { playlistHasSegments, playlistTargetDurationSecs } from "./playlist";
import { ensureStyles } from "./styles";
import type { PreviewStrings } from "./strings";
import type { PreviewEvent, PreviewHandle, PreviewRequest } from "./types";

/// 零段轮询超时(累计等待;常量锚,改值须过测试):daemon 慢产 init 走
/// 烧窗 503 诚实面(有界等待 10s),转码管线冷启面以 2 分钟为界诚实失败。
export const VIDEO_POLL_TIMEOUT_MS = 120_000;
/// 段 503 退避重试次数帽(每轮 ≤ CAP 10s;烧尽 = 失败面,不无限转)。
const SEG_RETRY_MAX = 8;

/// 定时器缝(测试手控;真面 setTimeout/clearTimeout)。
export interface VideoTimers {
  set(fn: () => void, ms: number): () => void;
}

const defaultTimers: VideoTimers = {
  set(fn, ms) {
    const id = setTimeout(fn, ms);
    return () => clearTimeout(id);
  },
};

/// XHR/Response 双形的网络细节读取(真面 hls.js 缺省 loader = XHR;测试假
/// 件同形。fetch loader 面 headers.get 兼容兜底)。
function statusOf(nd: unknown): number {
  const x = nd as { status?: unknown } | undefined;
  const n = x?.status;
  return typeof n === "number" ? n : 0;
}

function retryAfterSecsOf(nd: unknown): number | null {
  const x = nd as {
    getResponseHeader?: (name: string) => string | null;
    headers?: { get(name: string): string | null };
  } | null;
  const raw =
    typeof x?.getResponseHeader === "function"
      ? x.getResponseHeader("Retry-After")
      : (typeof x?.headers?.get === "function" ? x.headers.get("Retry-After") : null);
  const n = raw === null || raw === undefined ? Number.NaN : Number(raw);
  return Number.isFinite(n) && n > 0 ? n : null;
}

function bodyTextOf(nd: unknown): string {
  const x = nd as { responseText?: unknown; response?: unknown } | null;
  if (typeof x?.responseText === "string") return x.responseText;
  if (typeof x?.response === "string") return x.response;
  return "";
}

type ListenerCleanup = {
  target: EventTarget;
  type: string;
  fn: EventListener;
};

interface HlsErrorData {
  fatal: boolean;
  details: string;
  networkDetails?: unknown;
}

/// hls.js 假件/真面共同形(测试 vi.mock 同形注入;真类静态面取值)。
interface HlsLike {
  on(event: string, fn: (name: string, data: unknown) => void): void;
  loadSource(url: string): void;
  attachMedia(el: HTMLMediaElement): void;
  startLoad(): void;
  stopLoad(): void;
  destroy(): void;
}

interface HlsCtor {
  new (): HlsLike;
  isSupported(): boolean;
  Events: { ERROR: string; FRAG_LOADED: string; MANIFEST_PARSED: string };
  ErrorDetails: { FRAG_LOAD_ERROR: string; FRAG_LOAD_TIMEOUT: string };
}

export function mountVideo(
  el: HTMLElement,
  req: PreviewRequest,
  onEvent: (e: PreviewEvent) => void,
  strings: PreviewStrings,
  timers: VideoTimers = defaultTimers,
): PreviewHandle {
  ensureStyles();

  let destroyed = false;
  /// boot/重试代次:destroy 与重试皆翻代,迟火一律弃。
  let gen = 0;
  /// hls 实例(boot 成功后置;destroy 收口)。
  let hls: HlsLike | null = null;
  /// 段退避账(FRAG_LOADED 清;503 退避环自增)。
  let segRetries = 0;
  /// 在途定时器取消器(单枚:轮询环/退避环共用一道;换代即摘)。
  let cancelTimer: (() => void) | null = null;

  // ---- 装配:stage(video 海报帧打底)+ 失败/等待覆盖层 ----
  const root = document.createElement("div");
  root.className = "mtdss-preview mtdss-preview--video";

  const stage = document.createElement("div");
  stage.className = "mtdss-preview__stage mtdss-preview__stage--media";
  const video = document.createElement("video");
  video.className = "mtdss-preview__media";
  video.controls = true;
  video.preload = "metadata";
  if (req.urls.thumb !== undefined) video.setAttribute("poster", req.urls.thumb);
  stage.appendChild(video);
  root.appendChild(stage);

  const overlay = document.createElement("div");
  overlay.className = "mtdss-preview__overlay mtdss-preview__overlay--hidden";
  const spinner = document.createElement("div");
  spinner.className = "mtdss-preview__spinner";
  spinner.hidden = true;
  const note = document.createElement("div");
  note.className = "mtdss-preview__note";
  const guide = document.createElement("div");
  guide.className = "mtdss-preview__note";
  guide.textContent = strings.videoGuide;
  guide.hidden = true;
  const actions = document.createElement("div");
  actions.className = "mtdss-preview__actions";
  actions.hidden = true;
  const retryButton = document.createElement("button");
  retryButton.type = "button";
  retryButton.className = "mtdss-preview__button";
  retryButton.textContent = strings.retry;
  actions.appendChild(retryButton);
  if (req.urls.raw !== undefined) {
    const link = document.createElement("a");
    link.className = "mtdss-preview__download-link";
    link.href = req.urls.raw;
    link.download = req.meta.name;
    link.textContent = strings.downloadToView;
    actions.appendChild(link);
  }
  overlay.append(spinner, note, guide, actions);
  root.appendChild(overlay);
  el.appendChild(root);

  // ---- 监听记账:全走此门,destroy 一次摘净 ----
  const cleanups: ListenerCleanup[] = [];
  const listen = (target: EventTarget, type: string, fn: EventListener): void => {
    target.addEventListener(type, fn);
    cleanups.push({ target, type, fn });
  };

  // ---- 面态(三态互斥;spinner 面 = boot/轮询/退避共用等待词) ----
  const showWaiting = (): void => {
    overlay.classList.remove("mtdss-preview__overlay--hidden");
    spinner.hidden = false;
    note.textContent = strings.videoPreparing;
    guide.hidden = true;
    actions.hidden = true;
  };
  const showFailed = (message: string): void => {
    overlay.classList.remove("mtdss-preview__overlay--hidden");
    spinner.hidden = true;
    note.textContent = message;
    guide.hidden = false;
    actions.hidden = false;
    onEvent({ type: "retry_needed", message: strings.videoLoadFailed });
  };
  const hideOverlay = (): void => {
    overlay.classList.add("mtdss-preview__overlay--hidden");
  };

  const schedule = (fn: () => void, ms: number): void => {
    cancelTimer?.();
    cancelTimer = timers.set(() => {
      cancelTimer = null;
      fn();
    }, ms);
  };

  // ---- hls 错误环(503 seg_retry 退避 / source_offline / 致命态) ----
  const isFragLoadDetail = (hlsCtor: HlsCtor, details: string): boolean =>
    details === hlsCtor.ErrorDetails.FRAG_LOAD_ERROR ||
    details === hlsCtor.ErrorDetails.FRAG_LOAD_TIMEOUT;

  const handleHlsError = (hlsCtor: HlsCtor, data: HlsErrorData): void => {
    if (destroyed) return;
    const offline = statusOf(data.networkDetails) === 503 && bodyTextOf(data.networkDetails).includes("source_offline");
    if (data.fatal || offline) {
      // 致命态/源离线:海报帧打底 + 降级链引导 + 重试/下载面(重试钮走
      // 整体重启;源离线等自愈后一按即回)
      hls?.stopLoad();
      showFailed(strings.videoLoadFailed);
      return;
    }
    if (!isFragLoadDetail(hlsCtor, data.details) || hls === null) return;
    if (statusOf(data.networkDetails) !== 503) return; // 其余非致命 = hls 内建恢复
    segRetries += 1;
    if (segRetries > SEG_RETRY_MAX) {
      showFailed(strings.videoLoadFailed);
      return;
    }
    // 退避重试:停装载 → 对齐节律 → startLoad(重试中 spinner)
    hls.stopLoad();
    showWaiting();
    schedule(() => {
      if (destroyed) return;
      hls?.startLoad();
    }, segBackoffDelayMs(segRetries, retryAfterSecsOf(data.networkDetails)));
  };

  // ---- boot:playlist 预检(M7 零段轮询)→ hls.js 装载 ----
  /// 轮询预算(累计等待;跨 boot 续账——M7 轮询 = 同一预检环的多次重载,
  /// 重试钮 = 全新 boot 才清零)。
  let pollWaitedMs = 0;
  let pollAttempt = 0;
  const boot = async (): Promise<void> => {
    const myGen = ++gen;
    segRetries = 0;
    cancelTimer?.();
    cancelTimer = null;
    const url = req.urls.hls;
    if (url === undefined) {
      // 装配即不可预览:终态 error,无重试面
      spinner.remove();
      retryButton.remove();
      note.textContent = strings.unavailableMessage;
      overlay.classList.remove("mtdss-preview__overlay--hidden");
      onEvent({ type: "error", message: strings.unavailableMessage });
      return;
    }
    showWaiting();

    // M7 预检环:纯头 → 按 TARGETDURATION 轮询重载;请求失败 → seg 退避
    // 节律重试;首段出现 → 前进 hls;累计等待越界 → 失败面。
    for (;;) {
      let text: string;
      try {
        const res = await fetch(url, { cache: "no-store" });
        if (!res.ok) throw new Error(`playlist HTTP ${res.status}`);
        text = await (await res.blob()).text();
      } catch {
        if (destroyed || gen !== myGen) return;
        pollAttempt += 1;
        const delay = segBackoffDelayMs(pollAttempt);
        if (pollWaitedMs + delay > VIDEO_POLL_TIMEOUT_MS) {
          showFailed(strings.videoLoadFailed);
          return;
        }
        pollWaitedMs += delay;
        schedule(() => void boot(), delay);
        return;
      }
      if (destroyed || gen !== myGen) return;
      if (playlistHasSegments(text)) break;
      const intervalMs = playlistTargetDurationSecs(text) * 1000;
      if (pollWaitedMs + intervalMs > VIDEO_POLL_TIMEOUT_MS) {
        showFailed(strings.videoLoadFailed);
        return;
      }
      pollWaitedMs += intervalMs;
      schedule(() => void boot(), intervalMs);
      return;
    }

    // 段在册 → hls.js 装载(动态 import:不挂视频不付解析代价)
    const mod = (await import("hls.js")) as { default: unknown };
    const Hls = mod.default as HlsCtor;
    if (destroyed || gen !== myGen) return;
    if (!Hls.isSupported()) {
      // 能力缺(旧 WebView 无 MSE):降级链引导 + 下载打开(重试钮在场,
      // 装扩展/换端后不重开窗即可再试)
      showFailed(strings.videoLoadFailed);
      return;
    }
    const instance: HlsLike = new Hls();
    hls = instance;
    instance.on(Hls.Events.MANIFEST_PARSED, () => {
      if (destroyed) return;
      hideOverlay();
      onEvent({ type: "meta", bytes: req.meta.size });
    });
    instance.on(Hls.Events.FRAG_LOADED, () => {
      if (destroyed) return;
      segRetries = 0; // 恢复在途:退避账清零
    });
    instance.on(Hls.Events.ERROR, (_name, data) => {
      handleHlsError(Hls, data as HlsErrorData);
    });
    instance.attachMedia(video);
    instance.loadSource(url);
  };

  listen(retryButton, "click", () => {
    if (destroyed) return;
    hls?.destroy();
    hls = null;
    pollWaitedMs = 0; // 重试 = 全新预检环
    pollAttempt = 0;
    void boot();
  });

  // ---- boot 入口 ----
  void boot();

  // ---- destroy:定时器摘 + hls.destroy + 监听摘净 + 迟火弃 ----
  return {
    destroy: () => {
      if (destroyed) return;
      destroyed = true;
      gen += 1;
      cancelTimer?.();
      cancelTimer = null;
      hls?.destroy();
      hls = null;
      for (const entry of [...cleanups]) {
        entry.target.removeEventListener(entry.type, entry.fn);
      }
      root.remove();
    },
  };
}
