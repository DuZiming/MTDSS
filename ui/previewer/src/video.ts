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
///    503 携 `source_offline` 稳定词或连续退避烧尽(SEG_RETRY_MAX)→
///    stopLoad 装载停摆 + 失败面(源不再自愈面,重试环退出);
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
import type { PreviewStrings } from "./strings";
import type { PreviewEvent, PreviewHandle, PreviewRequest } from "./types";

/// 零段轮询超时(累计等待;常量锚,改值须过测试):daemon 慢产 init 走
/// 烧窗 503 诚实面(有界等待 10s),转码管线冷启面以 2 分钟为界诚实失败。
export const VIDEO_POLL_TIMEOUT_MS = 120_000;
/// 段 503 退避重试次数帽(**连续**循环;FRAG_LOADED/整体重试复位;烧尽 =
/// 失败面 + stopLoad 装载停摆,绝不无限转——对端半死时每循环都是一次
/// 503 往返,无帽会把对端锁面打满)。常量锚,改值须过测试。
export const SEG_RETRY_MAX = 6;

/// 直读元数据看门狗(r36):无 loadedmetadata 判失败窗(WebKit 对不可解
/// 容器可能干转不出 error——iOS 端 MKV 实测;格式不支持快提示)。常量锚。
export const RAW_META_TIMEOUT_MS = 12_000;

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

  let destroyed = false;
  /// boot/重试代次:destroy 与重试皆翻代,迟火一律弃。
  let gen = 0;
  /// hls 实例(boot 成功后置;destroy 收口)。
  let hls: HlsLike | null = null;
  /// 段退避账(FRAG_LOADED 清;503 退避环自增)。
  let segRetries = 0;
  /// 烧尽闩(退避帽到顶后置位):失败面已定,后续 503 不再放大账本、不再
  /// 重放 retry_needed;整体重试(boot)才复位。
  let segBurned = false;
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

  // ---- 原档直连兜底(field r14 #4:转码流 404/410 → <video> 直连 raw) ----
  /// 幂等门:一次尝试;重试钮重启 boot 时复位(重走 hls 预检)。loaded-
  /// metadata=直连成功(渐进播放);error=编解码不支持→失败面(HEVC 引导
  /// 词此境属实)。监听一次性自摘(先到者摘双)。
  /// r36 **元数据看门狗**:WebKit 对解不了的容器(如 iOS 端 MKV)可能
  /// 既不 error 也不出 metadata(干转)——RAW_META_TIMEOUT_MS 无
  /// loadedmetadata 即判失败出引导词(格式不支持快提示,用户裁定)。
  let rawDirectTried = false;
  const tryRawDirect = (): void => {
    const raw = req.urls.raw;
    if (rawDirectTried || raw === undefined) {
      showFailed(strings.videoLoadFailed);
      return;
    }
    rawDirectTried = true;
    showWaiting();
    let settled = false;
    const settle = (): void => {
      if (settled) return;
      settled = true;
      cancelTimer?.();
      cancelTimer = null;
    };
    const onMeta = (): void => {
      settle();
      video.removeEventListener("error", onErr);
      hideOverlay();
    };
    const onErr = (): void => {
      settle();
      video.removeEventListener("loadedmetadata", onMeta);
      showFailed(strings.videoLoadFailed);
    };
    video.addEventListener("loadedmetadata", onMeta, { once: true });
    video.addEventListener("error", onErr, { once: true });
    schedule(() => {
      if (settled) return;
      settled = true;
      video.removeEventListener("loadedmetadata", onMeta);
      video.removeEventListener("error", onErr);
      showFailed(strings.videoLoadFailed);
    }, RAW_META_TIMEOUT_MS);
    video.src = raw;
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
      // 整体重启;源离线等自愈后一按即回)。摘挂起定时器(若有在途退避/
      // 轮询钟,失败面后不得再 startLoad/重排环)
      cancelTimer?.();
      cancelTimer = null;
      hls?.stopLoad();
      showFailed(strings.videoLoadFailed);
      return;
    }
    if (!isFragLoadDetail(hlsCtor, data.details) || hls === null) return;
    if (statusOf(data.networkDetails) !== 503) return; // 其余非致命 = hls 内建恢复
    // r34:管线终败稳定词 = 立即失败面(不进退避环;iOS 转码器不可解容器
    // 的诚实终态,此前烧满重试预算 = 「一直加载中」)
    if (bodyTextOf(data.networkDetails).includes("管线已失败")) {
      cancelTimer?.();
      cancelTimer = null;
      hls.stopLoad();
      showFailed(strings.videoLoadFailed);
      return;
    }
    if (segBurned) return; // 烧尽闩:失败面已定,冗余 503 不再放大
    segRetries += 1;
    if (segRetries > SEG_RETRY_MAX) {
      // 烧尽:摘挂起退避钟 + 停装载(真面 hls.js 内建重试随之收口,绝不
      // 朝半死对端无限拉段;挂起的上一循环退避钟不得在失败面后再 startLoad)
      // → 失败面(降级链,重试钮整体重启即复位)
      segBurned = true;
      cancelTimer?.();
      cancelTimer = null;
      hls.stopLoad();
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
    segBurned = false; // 整体重试 = 全新退避环(帽重新起算)
    rawDirectTried = false; // 直连兜底随重启复位(重走 hls 预检)
    cancelTimer?.();
    cancelTimer = null;
    const url = req.urls.hls;
    if (url === undefined) {
      // r35 直读测试轮(用户裁定:全端撤转码消费,纯直读测格式支持面;
      // HLS 机器保留休眠——boot 侧不给 hls 地址即此形态,恢复=引导侧
      // 重新携带 urls.hls)。无 hls = 原档直连(loadedmetadata=成,
      // error=编码/容器不支持落失败面 + 引导词);重试钮可用。
      showWaiting();
      tryRawDirect();
      return;
    }
    showWaiting();

    // M7 预检环:纯头 → 按 TARGETDURATION 轮询重载;请求失败 → seg 退避
    // 节律重试;首段出现 → 前进 hls;累计等待越界 → 失败面。
    for (;;) {
      let text: string;
      try {
        const res = await fetch(url, { cache: "no-store" });
        // field 修复(2026-11-22 视频死转圈):404/410 = 永久面(桌面源
        // 无转码后端/句柄失效),立即失败——原逻辑把 404 当瞬时错误退避
        // 重试,要烧满 120s 预算才降级,用户看到的就是无限转圈。
        // field r14 #4:永久面先落**原档直连兜底**再谈失败——桌面 v1 无
        // 转码后端,但 WebView 系对 H.264/AAC 等兼容编码原生可播(<video>
        // 直连 raw,Range 续取渐进播放);编解码不支持才落失败面(此时
        // HEVC 扩展引导词才属实)。移动端转码在位不走此臂。
        if (res.status === 404 || res.status === 410) {
          if (destroyed || gen !== myGen) return;
          tryRawDirect();
          return;
        }
        if (!res.ok) {
          // r34:管线终败稳定词(转码器打不开容器等)= 不烧重试预算,
          // 立即失败面(MKV 在 iOS 转码器不可解,此前 120s 转圈)
          if (res.status === 503) {
            const body = await res.text().catch(() => "");
            if (body.includes("管线已失败") || body.includes("无视频轨")) {
              if (destroyed || gen !== myGen) return;
              showFailed(strings.videoLoadFailed);
              return;
            }
          }
          throw new Error(`playlist HTTP ${res.status}`);
        }
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
      hideOverlay(); // 段退避恢复面:spinner 摘(MANIFEST_PARSED 只发一次,不覆盖重试恢复)
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
