// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("hls.js", async () => {
  const { makeFakeHlsModule } = await import("./hls-fake");
  return makeFakeHlsModule();
});

import { fakeHlsState, type FakeHlsInstance, xhrLike } from "./hls-fake";
import { mountVideo, SEG_RETRY_MAX, VIDEO_POLL_TIMEOUT_MS } from "./video";
import { eventLog, settleUntil, unstubAll } from "./testutil";
import type { PreviewEvent, PreviewRequest } from "./types";
import { defaultPreviewStrings } from "./strings";

const HLS_URL = "http://127.0.0.1:1/f/h/video/index.m3u8?t=tok";
const RAW_URL = "http://127.0.0.1:1/f/h/raw";
const THUMB_URL = "http://127.0.0.1:1/f/h/thumb?w=256&t=tok";

/// 手控调度假件(timers 缝):set 记账,测试按需触发/检视延迟。
function manualTimers() {
  const pending: Array<{ fn: () => void; ms: number }> = [];
  return {
    pending,
    timers: {
      set(fn: () => void, ms: number): () => void {
        pending.push({ fn, ms });
        return () => {
          const i = pending.findIndex((t) => t.fn === fn);
          if (i >= 0) pending.splice(i, 1);
        };
      },
    },
    fireAll(): void {
      for (const t of [...pending]) t.fn();
      pending.length = 0;
    },
  };
}

/// fetch 假件(按调用序应答;文本面 = m3u8 playlist)。
function textResponse(text: string, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    blob: () => Promise.resolve(new Blob([text], { type: "application/vnd.apple.mpegurl" })),
  } as unknown as Response;
}

const HEADER_ONLY =
  '#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-PLAYLIST-TYPE:EVENT\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-MAP:URI="seg/0.m4s"\n';
const WITH_SEGS =
  "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-PLAYLIST-TYPE:EVENT\n#EXT-X-TARGETDURATION:4\n#EXT-X-MAP:URI=\"seg/0.m4s\"\n#EXTINF:4.000,\nseg/1.m4s\n";

function seqFetchText(responses: Response[]): ReturnType<typeof vi.fn> {
  const queue = [...responses];
  return vi.fn(() => {
    const res = queue.shift();
    if (res === undefined) return Promise.reject(new Error("unexpected extra fetch"));
    return Promise.resolve(res);
  });
}

function videoReq(urls: PreviewRequest["urls"]): PreviewRequest {
  return { kind: "video", urls, meta: { name: "影片.mp4", size: 1 << 20 } };
}

interface Mounted {
  el: HTMLElement;
  handle: { destroy(): void };
  events: PreviewEvent[];
  hls: () => FakeHlsInstance;
}

function mount(req: PreviewRequest, timers?: ReturnType<typeof manualTimers>): Mounted {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const log = eventLog<PreviewEvent>();
  const handle = mountVideo(el, req, log.onEvent, defaultPreviewStrings, timers?.timers);
  return {
    el,
    handle,
    events: log.events,
    hls: () => {
      if (fakeHlsState.instances.length === 0) throw new Error("hls 实例未创建");
      return fakeHlsState.instances[fakeHlsState.instances.length - 1];
    },
  };
}

beforeEach(() => {
  document.body.innerHTML = "";
  document.getElementById("mtdss-preview-styles")?.remove();
  fakeHlsState.reset();
});

afterEach(() => {
  unstubAll();
});

describe("mountVideo(hls.js 挂 urls.hls)", () => {
  it("装配形:video controls + 海报帧 urls.thumb;段在册 → hls 即建,loadSource=urls.hls、attachMedia=video 元素", async () => {
    vi.stubGlobal("fetch", seqFetchText([textResponse(WITH_SEGS)]));
    const view = mount(videoReq({ hls: HLS_URL, raw: RAW_URL, thumb: THUMB_URL }));
    await settleUntil(() => fakeHlsState.instances.length > 0);
    const video = view.el.querySelector("video");
    expect(video).not.toBeNull();
    expect(video!.hasAttribute("controls")).toBe(true);
    expect(video!.getAttribute("poster")).toBe(THUMB_URL);
    const hls = view.hls();
    expect(hls.url).toBe(HLS_URL);
    expect(hls.media).toBe(video);
    view.handle.destroy();
  });

  it("urls.hls 缺 = 装配即不可预览:终态 error,hls 零实例", async () => {
    const view = mount(videoReq({ raw: RAW_URL }));
    await settle();
    expect(view.events).toEqual([{ type: "error", message: defaultPreviewStrings.unavailableMessage }]);
    expect(fakeHlsState.instances.length).toBe(0);
    view.handle.destroy();
  });
});

describe("T15-M7 零段轮询(首刷纯头 → 按 TARGETDURATION 重载)", () => {
  it("首刷纯头无 EXTINF → 按 TARGETDURATION(4s)排轮询;段出现后 reload 前进到 hls(playlist 取两遍)", async () => {
    const timers = manualTimers();
    vi.stubGlobal("fetch", seqFetchText([textResponse(HEADER_ONLY), textResponse(WITH_SEGS)]));
    const view = mount(videoReq({ hls: HLS_URL, raw: RAW_URL, thumb: THUMB_URL }), timers);
    // 首刷:纯头 → 不建 hls,排轮询(等待面 = spinner 文案)
    await settleUntil(() => timers.pending.length > 0);
    expect(fakeHlsState.instances.length).toBe(0);
    expect(timers.pending[0]!.ms).toBe(4000);
    expect(view.el.textContent).toContain(defaultPreviewStrings.videoPreparing);
    // 重载:段出现 → hls 装配;manifest 就绪 → 等待面摘
    timers.fireAll();
    await settleUntil(() => fakeHlsState.instances.length > 0);
    expect(view.hls().url).toBe(HLS_URL);
    view.hls().trigger("hlsManifestParsed", {});
    await settle();
    expect(
      view.el.querySelector(".mtdss-preview__overlay")!.classList.contains("mtdss-preview__overlay--hidden"),
      "段在册 hls 装配后等待面摘",
    ).toBe(true);
    view.handle.destroy();
  });

  it("轮询超时(累计等待 ≥ 上限)→ 失败面(引导文案+重试钮)+ retry_needed;destroy 后环终止零迟火", async () => {
    const timers = manualTimers();
    vi.stubGlobal("fetch", seqFetchText(Array.from({ length: 64 }, () => textResponse(HEADER_ONLY))));
    const view = mount(videoReq({ hls: HLS_URL, raw: RAW_URL }), timers);
    await settleUntil(() => timers.pending.length > 0);
    // 连发至超上限(4s×30=120s):每发一轮又排一轮,直至累计等待越界
    for (let i = 0; i < Math.ceil(VIDEO_POLL_TIMEOUT_MS / 4000) + 1; i += 1) {
      timers.fireAll();
      await settle();
    }
    expect(timers.pending.length).toBe(0);
    expect(view.el.textContent).toContain(defaultPreviewStrings.videoLoadFailed);
    expect(view.el.querySelector("button.mtdss-preview__button")).not.toBeNull();
    expect(view.events.some((e) => e.type === "retry_needed")).toBe(true);
    view.handle.destroy();
  });
});

describe("503 seg_retry 消费(退避重试,spinner)", () => {
  it("fragLoadError 503(seg_retry 词 + Retry-After: 2)→ stopLoad + spinner;按 Retry-After 节律 startLoad;FRAG_LOADED 清账", async () => {
    vi.stubGlobal("fetch", seqFetchText([textResponse(WITH_SEGS)]));
    const timers = manualTimers();
    const view = mount(videoReq({ hls: HLS_URL, raw: RAW_URL }), timers);
    await settleUntil(() => fakeHlsState.instances.length > 0);
    const hls = view.hls();
    hls.trigger("hlsManifestParsed", {});
    hls.trigger("hlsError", {
      fatal: false,
      details: "fragLoadError",
      networkDetails: xhrLike(503, "seg_retry(窗烧尽)", "2"),
    });
    expect(hls.stopLoads).toBe(1);
    await settleUntil(() => timers.pending.length > 0);
    expect(timers.pending[0]!.ms).toBe(2000);
    expect(view.el.textContent).toContain(defaultPreviewStrings.videoPreparing);
    timers.fireAll();
    await settleUntil(() => hls.startLoads > 0);
    // 恢复:FRAG_LOADED 清退避账 + spinner 摘
    hls.trigger("hlsFragLoaded", {});
    hls.trigger("hlsError", {
      fatal: false,
      details: "fragLoadError",
      networkDetails: xhrLike(503, "seg_retry", null),
    });
    await settleUntil(() => timers.pending.length > 0);
    expect(timers.pending[0]!.ms).toBe(1000);
    view.handle.destroy();
  });

  it("Retry-After 缺席 = 指数退避(1s→2s);退避中 destroy → 定时器摘 + 零 startLoad 迟火", async () => {
    vi.stubGlobal("fetch", seqFetchText([textResponse(WITH_SEGS)]));
    const timers = manualTimers();
    const view = mount(videoReq({ hls: HLS_URL, raw: RAW_URL }), timers);
    await settleUntil(() => fakeHlsState.instances.length > 0);
    const hls = view.hls();
    hls.trigger("hlsError", {
      fatal: false,
      details: "fragLoadError",
      networkDetails: xhrLike(503, "seg_retry", null),
    });
    await settleUntil(() => timers.pending.length > 0);
    expect(timers.pending[0]!.ms).toBe(1000);
    view.handle.destroy();
    expect(timers.pending.length).toBe(0);
    timers.fireAll();
    await settle();
    expect(hls.startLoads).toBe(0);
  });

  it("退避恢复面:FRAG_LOADED → spinner 摘(MANIFEST_PARSED 只发一次,恢复面靠段事件收口)", async () => {
    vi.stubGlobal("fetch", seqFetchText([textResponse(WITH_SEGS)]));
    const timers = manualTimers();
    const view = mount(videoReq({ hls: HLS_URL, raw: RAW_URL }), timers);
    await settleUntil(() => fakeHlsState.instances.length > 0);
    const hls = view.hls();
    hls.trigger("hlsManifestParsed", {});
    hls.trigger("hlsError", {
      fatal: false,
      details: "fragLoadError",
      networkDetails: xhrLike(503, "seg_retry", "2"),
    });
    await settleUntil(() => timers.pending.length > 0);
    expect(
      view.el.querySelector(".mtdss-preview__overlay")!.classList.contains("mtdss-preview__overlay--hidden"),
    ).toBe(false);
    timers.fireAll();
    await settleUntil(() => hls.startLoads > 0);
    hls.trigger("hlsFragLoaded", {});
    await settle();
    expect(
      view.el.querySelector(".mtdss-preview__overlay")!.classList.contains("mtdss-preview__overlay--hidden"),
    ).toBe(true);
    view.handle.destroy();
  });

  it("503 source_offline 稳定词 → 不退避,直接失败面(引导+重试)+ retry_needed", async () => {
    vi.stubGlobal("fetch", seqFetchText([textResponse(WITH_SEGS)]));
    const view = mount(videoReq({ hls: HLS_URL, raw: RAW_URL }));
    await settleUntil(() => fakeHlsState.instances.length > 0);
    view.hls().trigger("hlsError", {
      fatal: false,
      details: "fragLoadError",
      networkDetails: xhrLike(503, "源离线: source_offline", "2"),
    });
    await settle();
    expect(view.el.textContent).toContain(defaultPreviewStrings.videoGuide);
    expect(view.el.querySelector("button.mtdss-preview__button")).not.toBeNull();
    expect(view.events.some((e) => e.type === "retry_needed")).toBe(true);
    view.handle.destroy();
  });

  it("对端半死 503 永续:连发 BURN+1 个退避循环 → 烧尽 = 失败面(引导+重试钮)+ 装载停摆(startLoad/stopLoad 收口、零 pending)、retry_needed 恰一次;烧尽后再来的 503 不再放大(face 幂等)", async () => {
    // BURN = 连续退避循环帽 = video.ts 导出锚 SEG_RETRY_MAX(常量锚:
    // 改值须过本测试)。对端死透时每一循环 = 一次 503 往返,无帽 = 无限
    // 拉段往返打满对端锁面(field 事故「整机卡死」的 UI 放大器)。
    const BURN = SEG_RETRY_MAX;
    vi.stubGlobal("fetch", seqFetchText([textResponse(WITH_SEGS)]));
    const timers = manualTimers();
    const view = mount(videoReq({ hls: HLS_URL, raw: RAW_URL, thumb: THUMB_URL }), timers);
    await settleUntil(() => fakeHlsState.instances.length > 0);
    const hls = view.hls();
    const err503 = () =>
      hls.trigger("hlsError", {
        fatal: false,
        details: "fragLoadError",
        networkDetails: xhrLike(503, "seg_retry(窗烧尽)", null),
      });
    // 烧帽:BURN 个循环,每循环 = 503 → 退避定时器 → fireAll(startLoad)
    for (let i = 0; i < BURN; i += 1) {
      err503();
      await settle();
      expect(timers.pending.length, `第 ${i + 1} 循环后应挂退避定时器`).toBe(1);
      timers.fireAll();
      await settle();
    }
    expect(hls.startLoads, "帽内每循环恰一次退避重开").toBe(BURN);
    expect(view.events.some((e) => e.type === "retry_needed"), "帽内不得失败").toBe(false);
    // 第 BURN+1 次 503:烧尽 → 失败面 + 装载停摆(不再排退避)
    err503();
    await settle();
    expect(timers.pending.length, "烧尽后不得再排退避定时器").toBe(0);
    expect(hls.stopLoads, "烧尽臂必须 stopLoad 收口装载(真面 hls 内建重试随之停)").toBe(BURN + 1);
    expect(view.el.textContent).toContain(defaultPreviewStrings.videoLoadFailed);
    expect(view.el.textContent).toContain(defaultPreviewStrings.videoGuide);
    expect(view.el.querySelector("button.mtdss-preview__button")).not.toBeNull();
    expect(view.events.filter((e) => e.type === "retry_needed"), "retry_needed 恰一次").toHaveLength(1);
    // 烧尽后再来的 503(真面 hls 已停,假件冗余发):不再放大账本/不再放事件
    err503();
    timers.fireAll();
    await settle();
    expect(hls.startLoads, "烧尽后 startLoad 恒不再增").toBe(BURN);
    expect(timers.pending.length).toBe(0);
    expect(view.events.filter((e) => e.type === "retry_needed")).toHaveLength(1);
    view.handle.destroy();
  });

  it("FRAG_LOADED 复位退避账:烧 3 → 恢复 → 重烧须再满帽才失败(非累计)", async () => {
    const BURN = SEG_RETRY_MAX;
    vi.stubGlobal("fetch", seqFetchText([textResponse(WITH_SEGS)]));
    const timers = manualTimers();
    const view = mount(videoReq({ hls: HLS_URL, raw: RAW_URL }), timers);
    await settleUntil(() => fakeHlsState.instances.length > 0);
    const hls = view.hls();
    const err503 = () =>
      hls.trigger("hlsError", {
        fatal: false,
        details: "fragLoadError",
        networkDetails: xhrLike(503, "seg_retry", null),
      });
    const cycle = async () => {
      err503();
      await settle();
      timers.fireAll();
      await settle();
    };
    // 前 3 循环 → 恢复(FRAG_LOADED 清账)
    for (let i = 0; i < 3; i += 1) await cycle();
    hls.trigger("hlsFragLoaded", {});
    // 复位后重烧帽内(BURN-1 循环):不得失败
    for (let i = 0; i < BURN - 1; i += 1) {
      await cycle();
      expect(view.events.some((e) => e.type === "retry_needed"), `复位后第 ${i + 1} 循环不得失败`).toBe(false);
    }
    expect(hls.startLoads).toBe(3 + (BURN - 1));
    // 第 BURN 循环(复位后连续第 6):烧尽 → 失败
    await cycle();
    err503();
    await settle();
    expect(timers.pending.length).toBe(0);
    expect(view.el.textContent).toContain(defaultPreviewStrings.videoLoadFailed);
    expect(view.events.some((e) => e.type === "retry_needed")).toBe(true);
    view.handle.destroy();
  });
});

describe("hls 致命态(降级链,spec §5.1)", () => {
  it("fatal error → 失败面:海报帧仍在 + 引导文案 + 重试钮 + 下载链接(raw)", async () => {
    vi.stubGlobal("fetch", seqFetchText([textResponse(WITH_SEGS)]));
    const view = mount(videoReq({ hls: HLS_URL, raw: RAW_URL, thumb: THUMB_URL }));
    await settleUntil(() => fakeHlsState.instances.length > 0);
    view.hls().trigger("hlsError", { fatal: true, details: "bufferStalledError" });
    await settle();
    expect(view.el.querySelector("video")!.getAttribute("poster")).toBe(THUMB_URL);
    expect(view.el.textContent).toContain(defaultPreviewStrings.videoLoadFailed);
    expect(view.el.textContent).toContain(defaultPreviewStrings.videoGuide);
    const a = view.el.querySelector<HTMLAnchorElement>("a.mtdss-preview__download-link");
    expect(a).not.toBeNull();
    expect(a!.getAttribute("href")).toBe(RAW_URL);
    expect(view.events.some((e) => e.type === "retry_needed")).toBe(true);
    view.handle.destroy();
  });

  it("manifestParsed → meta 事件(主内容就绪,bytes=meta.size)", async () => {
    vi.stubGlobal("fetch", seqFetchText([textResponse(WITH_SEGS)]));
    const view = mount(videoReq({ hls: HLS_URL, raw: RAW_URL }));
    await settleUntil(() => fakeHlsState.instances.length > 0);
    view.hls().trigger("hlsManifestParsed", {});
    await settle();
    expect(view.events).toEqual([{ type: "meta", bytes: 1 << 20 }]);
    view.handle.destroy();
  });
});

describe("段 URI 绝对+令牌透过面(T21 fix round 1 Critical #1 消费端钉面)", () => {
  it("daemon 绝对+?t= 段行 playlist → loadSource 原样透传(恒不重写;绝对段行 EXTINF 判别照过)", async () => {
    // 生产形复刻(daemon playlist_m3u8 输出):MAP/EXTINF 段行 = 绝对 URI 携 ?t=
    const segBase = HLS_URL.replace("index.m3u8?t=tok", "seg");
    const abs =
      "#EXTM3U\n#EXT-X-TARGETDURATION:4\n" +
      `#EXT-X-MAP:URI="${segBase}/0.m4s?t=tok"\n` +
      `#EXTINF:4.000,\n${segBase}/1.m4s?t=tok\n`;
    vi.stubGlobal("fetch", seqFetchText([textResponse(abs)]));
    const view = mount(videoReq({ hls: HLS_URL, raw: RAW_URL }));
    await settleUntil(() => fakeHlsState.instances.length > 0);
    // 消费端唯一职责 = 原样透传:URL 组装单点(lib/preview)已携 ?t=,
    // playlist 段行绝对面由 daemon 生成,本包不解析不重写(身份断言)
    expect(view.hls().url).toBe(HLS_URL);
    view.handle.destroy();
  });
});

describe("Hls.isSupported() 门", () => {
  it("不支持 → 失败面(引导+下载链接),hls 零实例", async () => {
    fakeHlsState.supported = false;
    vi.stubGlobal("fetch", seqFetchText([textResponse(WITH_SEGS)]));
    const view = mount(videoReq({ hls: HLS_URL, raw: RAW_URL }));
    await settle();
    expect(fakeHlsState.instances.length).toBe(0);
    expect(view.el.textContent).toContain(defaultPreviewStrings.videoGuide);
    expect(view.el.querySelector("a.mtdss-preview__download-link")).not.toBeNull();
    view.handle.destroy();
  });
});

describe("destroy 收口", () => {
  it("destroy → hls.destroy + 装配 DOM 摘净", async () => {
    vi.stubGlobal("fetch", seqFetchText([textResponse(WITH_SEGS)]));
    const view = mount(videoReq({ hls: HLS_URL, raw: RAW_URL }));
    await settleUntil(() => fakeHlsState.instances.length > 0);
    const hls = view.hls();
    view.handle.destroy();
    expect(hls.destroyed).toBe(true);
    expect(view.el.querySelector(".mtdss-preview--video")).toBeNull();
  });
});

async function settle(): Promise<void> {
  await new Promise((r) => setTimeout(r, 0));
}
