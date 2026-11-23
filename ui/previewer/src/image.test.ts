// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { mountPreview } from "./router";
import {
  blob,
  deferred,
  errResponse,
  eventLog,
  JPEG_URL,
  mousePointer,
  okResponse,
  RAW_URL,
  readTransform,
  settle,
  stubFetch,
  stubObjectUrls,
  THUMB_URL,
  unstubAll,
  wheelEvent,
  type Deferred,
  type ObjectUrlStub,
} from "./testutil";
import type { PreviewEvent, PreviewRequest } from "./types";

function imageReq(urls: PreviewRequest["urls"], name = "照片.jpg"): PreviewRequest {
  return { kind: "image", urls, meta: { name, size: 4096 } };
}

let urls: ObjectUrlStub;

beforeEach(() => {
  document.body.innerHTML = "";
  urls = stubObjectUrls();
});

afterEach(() => {
  unstubAll();
});

function mount(req: PreviewRequest, events = eventLog<PreviewEvent>()) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const handle = mountPreview(el, req, events.onEvent);
  const img = () => el.querySelector<HTMLImageElement>("img.mtdss-preview__img");
  const overlay = () => el.querySelector<HTMLElement>(".mtdss-preview__overlay");
  const retryButton = () => el.querySelector<HTMLButtonElement>(".mtdss-preview__overlay button");
  return { el, handle, img, overlay, retryButton, events: events.events };
}

describe("图片渐进态机:thumb → raw", () => {
  it("thumb→raw ok:raw 就绪后替换缩略图,revoke 旧 objectURL,meta 携 raw 字节数", async () => {
    const thumbBlob = blob(8, "image/jpeg");
    const rawBlob = blob(100, "image/png");
    stubFetch({ [THUMB_URL]: okResponse(thumbBlob), [RAW_URL]: okResponse(rawBlob) });
    const view = mount(imageReq({ thumb: THUMB_URL, raw: RAW_URL }));

    await settle();
    // 渐进:两度走字节桥——先缩略图后原图,终面=原图;旧面已回收
    expect(view.img()!.src).toBe(urls.urlOf(rawBlob));
    expect(urls.revoked).toEqual([urls.urlOf(thumbBlob)]);
    expect(view.events).toEqual([{ type: "meta", bytes: 100 }]);
    // 失败覆盖层不出(隐藏态)
    expect(view.overlay()!.classList.contains("mtdss-preview__overlay--hidden")).toBe(true);
    view.handle.destroy();
  });

  it("thumb 缺席:直接 raw,同样替换+meta", async () => {
    const rawBlob = blob(64);
    stubFetch({ [RAW_URL]: okResponse(rawBlob) });
    const view = mount(imageReq({ raw: RAW_URL }));
    await settle();
    expect(view.img()!.src).toBe(urls.urlOf(rawBlob));
    expect(view.events).toEqual([{ type: "meta", bytes: 64 }]);
    view.handle.destroy();
  });

  it("thumb 失败为尽力而为:静默落 raw,不与 raw 失败共词", async () => {
    const rawBlob = blob(16);
    stubFetch({ [THUMB_URL]: errResponse(500), [RAW_URL]: okResponse(rawBlob) });
    const view = mount(imageReq({ thumb: THUMB_URL, raw: RAW_URL }));
    await settle();
    expect(view.img()!.src).toBe(urls.urlOf(rawBlob));
    expect(view.events).toEqual([{ type: "meta", bytes: 16 }]);
    view.handle.destroy();
  });

  it("HEIC 转码面:urls.jpeg 优先于 raw(单请求,raw 不拉)", async () => {
    const jpegBlob = blob(32);
    const fetchMock = stubFetch({ [JPEG_URL]: okResponse(jpegBlob), [RAW_URL]: okResponse(blob(999)) });
    const view = mount(imageReq({ jpeg: JPEG_URL, raw: RAW_URL }));
    await settle();
    expect(view.img()!.src).toBe(urls.urlOf(jpegBlob));
    expect(view.events).toEqual([{ type: "meta", bytes: 32 }]);
    const fetchedUrls = fetchMock.mock.calls.map((c) => c[0]);
    expect(fetchedUrls).toEqual([JPEG_URL]);
    view.handle.destroy();
  });

  it("raw 迟到于缩略图:缩略图不被迟到旧面覆回(渐进只进不退)", async () => {
    const thumbBlob = blob(8);
    const rawBlob = blob(100);
    const pending: Deferred<Response> = deferred();
    const fetchMock = stubFetch({ [THUMB_URL]: okResponse(thumbBlob) });
    fetchMock.mockImplementation((url: unknown) => {
      if (url === THUMB_URL) return Promise.resolve(okResponse(thumbBlob));
      if (url === RAW_URL) return pending.promise;
      return Promise.reject(new Error(`unexpected fetch: ${String(url)}`));
    });
    const view = mount(imageReq({ thumb: THUMB_URL, raw: RAW_URL }));
    await settle();
    expect(view.img()!.src).toBe(urls.urlOf(thumbBlob));
    // raw 在缩略图已显后才落——终面恒原图,不得被竞窗翻回缩略图
    pending.resolve(okResponse(rawBlob));
    await settle();
    expect(view.img()!.src).toBe(urls.urlOf(rawBlob));
    view.handle.destroy();
  });
});

describe("raw 失败:停缩略图态+重试钮(retry_needed)", () => {
  it("raw fail:缩略图停驻、覆盖层+重试钮出、retry_needed 事件恰一枚", async () => {
    const thumbBlob = blob(8);
    stubFetch({ [THUMB_URL]: okResponse(thumbBlob), [RAW_URL]: errResponse(503) });
    const view = mount(imageReq({ thumb: THUMB_URL, raw: RAW_URL }));
    await settle();
    expect(view.img()!.src).toBe(urls.urlOf(thumbBlob)); // 停缩略图态
    expect(view.overlay()!.classList.contains("mtdss-preview__overlay--hidden")).toBe(false);
    expect(view.retryButton()?.textContent).toBe("重试");
    expect(view.events).toEqual([{ type: "retry_needed", message: expect.any(String) }]);
    view.handle.destroy();
  });

  it("重试后源恢复:重拉 raw 成功 → 替换+meta、覆盖层收、不重复 retry_needed", async () => {
    const thumbBlob = blob(8);
    const rawBlob = blob(100);
    const responses: Record<string, Response> = {
      [THUMB_URL]: okResponse(thumbBlob),
      [RAW_URL]: errResponse(503),
    };
    const fetchMock = stubFetch(responses);
    const view = mount(imageReq({ thumb: THUMB_URL, raw: RAW_URL }));
    await settle();
    expect(view.events).toEqual([{ type: "retry_needed", message: expect.any(String) }]);

    responses[RAW_URL] = okResponse(rawBlob); // 源端恢复
    view.retryButton()!.click();
    await settle();
    expect(view.img()!.src).toBe(urls.urlOf(rawBlob));
    expect(view.overlay()!.classList.contains("mtdss-preview__overlay--hidden")).toBe(true);
    expect(view.events).toEqual([
      { type: "retry_needed", message: expect.any(String) },
      { type: "meta", bytes: 100 },
    ]);
    expect(fetchMock.mock.calls.filter((c) => c[0] === RAW_URL).length).toBe(2);
    view.handle.destroy();
  });

  it("重试再失败:retry_needed 逐次重发,缩略图态持续", async () => {
    const thumbBlob = blob(8);
    stubFetch({ [THUMB_URL]: okResponse(thumbBlob), [RAW_URL]: errResponse(503) });
    const view = mount(imageReq({ thumb: THUMB_URL, raw: RAW_URL }));
    await settle();
    view.retryButton()!.click();
    await settle();
    view.retryButton()!.click();
    await settle();
    expect(view.events.filter((e) => e.type === "retry_needed").length).toBe(3); // 首拉+两次重试
    expect(view.img()!.src).toBe(urls.urlOf(thumbBlob));
    view.handle.destroy();
  });
});

describe("事件与降级边界", () => {
  it("无任何 url:error 事件+零 fetch", async () => {
    const fetchMock = stubFetch({});
    const view = mount(imageReq({}));
    await settle();
    expect(view.events).toEqual([{ type: "error", message: expect.any(String) }]);
    expect(fetchMock).not.toHaveBeenCalled();
    view.handle.destroy();
  });
});

describe("destroy 清理:无泄漏监听/无迟火回调/字节桥回收", () => {
  it("destroy 后 wheel 不再缩放、拖拽不再平移(监听摘净)", async () => {
    const rawBlob = blob(16);
    stubFetch({ [RAW_URL]: okResponse(rawBlob) });
    const view = mount(imageReq({ raw: RAW_URL }));
    await settle();
    const stage = view.el.querySelector<HTMLElement>(".mtdss-preview__stage")!;
    const imgEl = view.img()!;
    view.handle.destroy();

    expect(view.el.querySelector(".mtdss-preview")).toBeNull(); // 装配 DOM 已摘净
    const before = imgEl.style.transform;
    stage.dispatchEvent(wheelEvent(-120));
    expect(imgEl.style.transform).toBe(before); // 缩放监听已摘
    window.dispatchEvent(mousePointer("pointermove", 100, 100));
    expect(imgEl.style.transform).toBe(before); // 拖拽窗面监听已摘
  });

  it("raw 在途中 destroy:请求被 abort、迟火响应被代次弃、零新 objectURL/零事件", async () => {
    const thumbBlob = blob(8);
    const rawBlob = blob(100);
    const pending: Deferred<Response> = deferred();
    const fetchMock = stubFetch({ [THUMB_URL]: okResponse(thumbBlob) });
    fetchMock.mockImplementation((url: unknown) => {
      if (url === THUMB_URL) return Promise.resolve(okResponse(thumbBlob));
      if (url === RAW_URL) return pending.promise;
      return Promise.reject(new Error(`unexpected fetch: ${String(url)}`));
    });
    const view = mount(imageReq({ thumb: THUMB_URL, raw: RAW_URL }));
    await settle();
    expect(view.img()!.src).toBe(urls.urlOf(thumbBlob));
    expect(urls.created.length).toBe(1);

    view.handle.destroy();
    expect(fetchMock.mock.calls.find((c) => c[0] === RAW_URL)?.[1]?.signal?.aborted).toBe(true);
    expect(view.el.querySelector(".mtdss-preview")).toBeNull();
    expect(urls.revoked).toEqual([urls.urlOf(thumbBlob)]); // 在用字节桥已回收

    pending.resolve(okResponse(rawBlob)); // 迟火
    await settle();
    expect(urls.created.length).toBe(1); // 未为 raw 开新桥
    expect(view.events).toEqual([]); // 零事件外泄
  });
});

describe("zoom/pan 手势:滚轮缩放+鼠标拖拽平移", () => {
  it("滚轮:deltaY<0 放大、>0 缩回 1;缩回原大平移归零;放大有上界", () => {
    stubFetch({});
    const view = mount(imageReq({ raw: RAW_URL }));
    const stage = view.el.querySelector<HTMLElement>(".mtdss-preview__stage")!;

    stage.dispatchEvent(wheelEvent(-120));
    const z1 = readTransform(view.img()!).z;
    expect(z1).toBeGreaterThan(1);

    for (let i = 0; i < 30; i += 1) stage.dispatchEvent(wheelEvent(-120));
    expect(readTransform(view.img()!).z).toBe(8); // 上界封顶

    stage.dispatchEvent(wheelEvent(100000)); // 满幅缩回:钳回原大且平移归零
    const reset = readTransform(view.img()!);
    expect(reset.z).toBe(1);
    expect(reset.x).toBe(0);
    expect(reset.y).toBe(0);
    view.handle.destroy();
  });

  it("拖拽:按下-移动-抬起平移;抬起后移动无效;未放大不平移", () => {
    stubFetch({});
    const view = mount(imageReq({ raw: RAW_URL }));
    const stage = view.el.querySelector<HTMLElement>(".mtdss-preview__stage")!;

    // zoom=1:平移门关(图未放大,平移无意义)
    stage.dispatchEvent(mousePointer("pointerdown", 10, 10));
    window.dispatchEvent(mousePointer("pointermove", 50, 60));
    expect(readTransform(view.img()!)).toEqual({ x: 0, y: 0, z: 1 });

    stage.dispatchEvent(wheelEvent(-120)); // 放大开拖拽门
    stage.dispatchEvent(mousePointer("pointerdown", 10, 10));
    window.dispatchEvent(mousePointer("pointermove", 40, 50));
    window.dispatchEvent(mousePointer("pointermove", 60, 70));
    const panned = readTransform(view.img()!);
    expect(panned).toEqual({ x: 50, y: 60, z: panned.z });

    window.dispatchEvent(mousePointer("pointerup", 60, 70)); // 抬起
    window.dispatchEvent(mousePointer("pointermove", 500, 500));
    expect(readTransform(view.img()!)).toEqual(panned); // 松手即停

    view.handle.destroy();
  });

  it("拖拽途中 destroy:窗面移动监听已摘,平移不走", () => {
    stubFetch({});
    const view = mount(imageReq({ raw: RAW_URL }));
    const stage = view.el.querySelector<HTMLElement>(".mtdss-preview__stage")!;
    const imgEl = view.img()!;
    stage.dispatchEvent(wheelEvent(-120));
    const before = readTransform(imgEl);

    stage.dispatchEvent(mousePointer("pointerdown", 10, 10));
    view.handle.destroy();
    window.dispatchEvent(mousePointer("pointermove", 300, 400));
    expect(readTransform(imgEl)).toEqual(before);
    expect(view.el.querySelector(".mtdss-preview")).toBeNull();
  });

  it("触摸(pointerType=touch)不平移:手势留给移动端容器自理", () => {
    stubFetch({});
    const view = mount(imageReq({ raw: RAW_URL }));
    const stage = view.el.querySelector<HTMLElement>(".mtdss-preview__stage")!;
    stage.dispatchEvent(wheelEvent(-120));
    const before = readTransform(view.img()!);

    const touchDown = new PointerEvent("pointerdown", {
      clientX: 10,
      clientY: 10,
      pointerType: "touch",
      button: 0,
      cancelable: true,
    });
    stage.dispatchEvent(touchDown);
    const touchMove = new PointerEvent("pointermove", {
      clientX: 300,
      clientY: 400,
      pointerType: "touch",
      cancelable: true,
    });
    window.dispatchEvent(touchMove);
    expect(readTransform(view.img()!)).toEqual(before);
    view.handle.destroy();
  });
});
