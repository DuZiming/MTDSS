// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { mountAudio } from "./audio";
import { eventLog, RAW_URL, settle, unstubAll } from "./testutil";
import type { PreviewEvent, PreviewRequest } from "./types";
import { defaultPreviewStrings } from "./strings";

const RAW = RAW_URL;

function audioReq(urls: PreviewRequest["urls"]): PreviewRequest {
  return { kind: "audio", urls, meta: { name: "曲目.mp3", size: 1 << 16 } };
}

function mount(req: PreviewRequest) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const log = eventLog<PreviewEvent>();
  const handle = mountAudio(el, req, log.onEvent, defaultPreviewStrings);
  return { el, handle, events: log.events };
}

beforeEach(() => {
  document.body.innerHTML = "";});

afterEach(() => {
  unstubAll();
});

describe("mountAudio(HTML5 raw 直连)", () => {
  it("装配形:audio controls src=urls.raw", () => {
    const view = mount(audioReq({ raw: RAW }));
    const audio = view.el.querySelector("audio");
    expect(audio).not.toBeNull();
    expect(audio!.hasAttribute("controls")).toBe(true);
    expect(audio!.getAttribute("src")).toBe(RAW);
    view.handle.destroy();
  });

  it("urls.raw 缺 = 装配即不可预览:终态 error,零 audio 元素", () => {
    const view = mount(audioReq({}));
    expect(view.events).toEqual([{ type: "error", message: defaultPreviewStrings.unavailableMessage }]);
    expect(view.el.querySelector("audio")).toBeNull();
    view.handle.destroy();
  });

  it("错误面:audio error → 覆盖层失败词+重试钮 + retry_needed 事件(image 房法);重试点击摘覆盖层", async () => {
    const view = mount(audioReq({ raw: RAW }));
    const audio = view.el.querySelector("audio")!;
    audio.dispatchEvent(new Event("error"));
    await settle();
    expect(view.el.textContent).toContain(defaultPreviewStrings.audioLoadFailed);
    expect(view.el.querySelector("button.mtdss-preview__button")).not.toBeNull();
    expect(view.events).toEqual([
      { type: "retry_needed", message: defaultPreviewStrings.audioRetryNeeded },
    ]);
    const loadSpy = vi.spyOn(audio, "load").mockImplementation(() => {});
    view.el.querySelector<HTMLButtonElement>("button.mtdss-preview__button")!.click();
    await settle();
    expect(loadSpy).toHaveBeenCalledTimes(1);
    expect(
      view.el.querySelector(".mtdss-preview__overlay")!.classList.contains("mtdss-preview__overlay--hidden"),
      "重试后覆盖层隐(重试钮随层藏,非摘元素)",
    ).toBe(true);
    view.handle.destroy();
  });

  it("就绪面:loadedmetadata → meta 事件(bytes=meta.size)且覆盖层不显", async () => {
    const view = mount(audioReq({ raw: RAW }));
    const audio = view.el.querySelector("audio")!;
    audio.dispatchEvent(new Event("loadedmetadata"));
    await settle();
    expect(view.events).toEqual([{ type: "meta", bytes: 1 << 16 }]);
    view.handle.destroy();
  });

  it("destroy → 装配 DOM 摘净", () => {
    const view = mount(audioReq({ raw: RAW }));
    view.handle.destroy();
    expect(view.el.querySelector(".mtdss-preview--audio")).toBeNull();
  });
});
