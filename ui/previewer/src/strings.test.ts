// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { mountPreview } from "./router";
import {
  defaultPreviewStrings,
  formatPdfPageIndicator,
  resolvePreviewStrings,
} from "./strings";
import { errResponse, eventLog, okResponse, blob, RAW_URL, settle, stubFetch, unstubAll } from "./testutil";
import type { PreviewEvent, PreviewRequest } from "./types";

function req(
  kind: PreviewRequest["kind"],
  urls: PreviewRequest["urls"] = {},
  name = "文件.bin",
  strings?: PreviewRequest["strings"],
): PreviewRequest {
  return { kind, urls, meta: { name, size: 1234 }, ...(strings === undefined ? {} : { strings }) };
}

function textOf(el: HTMLElement): string {
  return (el.textContent ?? "").trim();
}

function mount(reqBody: PreviewRequest) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const events = eventLog<PreviewEvent>();
  const handle = mountPreview(el, reqBody, events.onEvent);
  return { el, handle, events: events.events };
}

beforeEach(() => {
  document.body.innerHTML = "";
  document.getElementById("mtdss-preview-styles")?.remove();
});

afterEach(() => {
  unstubAll();
});

describe("文案容器缝(T16-M1):缺省形", () => {
  it("缺省=中文文案:占位出「该格式暂未支持」,raw 在出「下载后查看」链接", () => {
    const view = mount(req("video", { raw: RAW_URL }, "影片.mkv"));
    expect(textOf(view.el)).toContain("该格式暂未支持");
    const a = view.el.querySelector<HTMLAnchorElement>("a.mtdss-preview__download-link");
    expect(a?.textContent).toBe("下载后查看");
    view.handle.destroy();
  });

  it("缺省:image 失败面出「重试」钮与「原图加载失败」事件词", async () => {
    stubFetch({ [RAW_URL]: errResponse(503) });
    const view = mount(req("image", { raw: RAW_URL }, "照片.jpg"));
    await settle();
    expect(view.el.querySelector<HTMLButtonElement>("button.mtdss-preview__button")?.textContent).toBe(
      defaultPreviewStrings.retry,
    );
    expect(view.events).toEqual([{ type: "retry_needed", message: defaultPreviewStrings.imageRetryNeeded }]);
    view.handle.destroy();
  });

  it("缺省:image 渐进成功 meta 不携页码页数(pdf 扩展对旧消费者零触碰)", async () => {
    stubFetch({ [RAW_URL]: okResponse(blob(16)) });
    const view = mount(req("image", { raw: RAW_URL }, "照片.jpg"));
    await settle();
    expect(view.events).toEqual([{ type: "meta", bytes: 16 }]);
    view.handle.destroy();
  });
});

describe("文案容器缝:覆写形", () => {
  it("部分覆写生效,未覆写键仍缺省", () => {
    const view = mount(
      req("video", {}, "影片.mkv", { unsupportedNote: "Not supported", downloadHint: "Save first" }),
    );
    expect(textOf(view.el)).toContain("Not supported");
    expect(textOf(view.el)).toContain("Save first");
    expect(textOf(view.el)).not.toContain("该格式暂未支持");
    view.handle.destroy();
  });

  it("覆写穿透图片查看器:重试钮文案替换", async () => {
    stubFetch({ [RAW_URL]: errResponse(503) });
    const view = mount(req("image", { raw: RAW_URL }, "照片.jpg", { retry: "Retry" }));
    await settle();
    expect(view.el.querySelector<HTMLButtonElement>("button.mtdss-preview__button")?.textContent).toBe(
      "Retry",
    );
    view.handle.destroy();
  });
});

describe("resolvePreviewStrings / formatPdfPageIndicator(单元)", () => {
  it("undefined → 全缺省;partial → 合并(逐键覆盖)", () => {
    expect(resolvePreviewStrings(undefined)).toEqual(defaultPreviewStrings);
    const merged = resolvePreviewStrings({ retry: "重试!", pdfLoadMore: "More" });
    expect(merged.retry).toBe("重试!");
    expect(merged.pdfLoadMore).toBe("More");
    expect(merged.textLoadMore).toBe(defaultPreviewStrings.textLoadMore);
    expect(merged.imageLoadFailed).toBe(defaultPreviewStrings.imageLoadFailed);
  });

  it("页码模板:{page}/{total} 词位全量替换;覆写模板同径", () => {
    expect(formatPdfPageIndicator("第 {page} / {total} 页", 3, 9)).toBe("第 3 / 9 页");
    expect(formatPdfPageIndicator("p{page}of{total}", 12, 40)).toBe("p12of40");
    expect(formatPdfPageIndicator("第 {page} / {total} 页(共 {total} 页)", 2, 5)).toBe(
      "第 2 / 5 页(共 5 页)",
    );
  });
});
