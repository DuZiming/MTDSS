// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { mountPreview } from "./router";
import { defaultPreviewStrings } from "./strings";
import {
  bytesResponse,
  deferred,
  errResponse,
  eventLog,
  rangeHeaderOf,
  RAW_URL,
  sequentialFetch,
  settle,
  settleUntil,
  stubFetch,
  unstubAll,
} from "./testutil";
import { TEXT_HEAD_BYTES } from "./text";
import type { PreviewEvent, PreviewRequest } from "./types";

// 消毒断言环境注记:DOMPurify 在 happy-dom 下 isSupported=true 但产物损坏
// (h1 丢失/script 残留)——其模板解析依赖 jsdom 级 DOM 行为;XSS 消毒钉面必须
// 跑在 jsdom(生产=WebView2/WKWebView 真浏览器面,不受此限)。本文件整体 jsdom。

function textReq(
  kind: "md" | "text",
  urls: PreviewRequest["urls"],
  name: string,
  strings?: PreviewRequest["strings"],
): PreviewRequest {
  return { kind, urls, meta: { name, size: 1 }, ...(strings === undefined ? {} : { strings }) };
}

function mount(req: PreviewRequest, events = eventLog<PreviewEvent>()) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const handle = mountPreview(el, req, events.onEvent);
  const mdBody = () => el.querySelector<HTMLElement>(".mtdss-preview__md");
  const code = () => el.querySelector<HTMLElement>("code.mtdss-preview__code-code");
  const moreButton = () => el.querySelector<HTMLButtonElement>("button.mtdss-preview__text-more");
  const overlay = () => el.querySelector<HTMLElement>(".mtdss-preview__overlay");
  const retryButton = () => el.querySelector<HTMLButtonElement>(".mtdss-preview__overlay button");
  return { el, handle, events: events.events, mdBody, code, moreButton, overlay, retryButton };
}

/// 头部窗恰 64KB 的字节形(前缀 md 正文,余量 x 填充)。
function headWindow(prefixMd: string): Uint8Array {
  const prefix = new TextEncoder().encode(prefixMd);
  const data = new Uint8Array(TEXT_HEAD_BYTES);
  data.set(prefix, 0);
  data.fill(0x78, prefix.length); // "x"
  return data;
}

function errBytesResponse(): Response {
  return errResponse(503);
}

beforeEach(() => {
  document.body.innerHTML = "";
  document.getElementById("mtdss-preview-styles")?.remove();
});

afterEach(() => {
  unstubAll();
});

describe("md:64KB 渐进与渲染", () => {
  it("小文件:头部单请求(Range bytes=0-65535)即全量,渲染+meta,无续读钮", async () => {
    const src = "# 标题\n\n正文 **加粗** 段落\n";
    const data = new TextEncoder().encode(src);
    const fetchMock = sequentialFetch([bytesResponse(data, { begin: 0, total: data.length })]);

    const view = mount(textReq("md", { raw: RAW_URL }, "笔记.md"));
    await settleUntil(() => view.mdBody()?.querySelector("h1") !== null);

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(rangeHeaderOf(fetchMock.mock.calls[0] as unknown[])).toBe(`bytes=0-${TEXT_HEAD_BYTES - 1}`);
    // markdown 渲染:h1/strong 到位
    expect(view.mdBody()?.querySelector("h1")?.textContent).toBe("标题");
    expect(view.mdBody()?.querySelector("strong")?.textContent).toBe("加粗");
    expect(view.events).toEqual([{ type: "meta", bytes: data.length }]);
    expect(view.moreButton()).toBeNull(); // 已读尽:续读钮不装配
    view.handle.destroy();
  });

  it("大文件:头部窗后出「加载更多」,点击续读恰一窗,读完钮移除", async () => {
    const head = headWindow("# 头部\n");
    const tail = new TextEncoder().encode("\n## 续段\n");
    const total = head.length + tail.length;
    const fetchMock = sequentialFetch([
      bytesResponse(head, { begin: 0, total }),
      bytesResponse(tail, { begin: TEXT_HEAD_BYTES, total }),
    ]);

    const view = mount(textReq("md", { raw: RAW_URL }, "长文.md"));
    await settleUntil(() => view.mdBody()?.querySelector("h1") !== null);
    expect(view.mdBody()?.querySelector("h1")?.textContent).toBe("头部");
    expect(view.moreButton()?.textContent).toBe(defaultPreviewStrings.textLoadMore);
    expect(view.events).toEqual([{ type: "meta", bytes: total }]);

    view.moreButton()!.click();
    await settleUntil(() => view.mdBody()?.querySelector("h2") !== null);

    // 续读:恰一枚请求,窗=64KB;尾部渲染到位;读完钮移除
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(rangeHeaderOf(fetchMock.mock.calls[1] as unknown[])).toBe(
      `bytes=${TEXT_HEAD_BYTES}-${TEXT_HEAD_BYTES + TEXT_HEAD_BYTES - 1}`,
    );
    expect(view.mdBody()?.querySelector("h2")?.textContent).toBe("续段");
    expect(view.moreButton()).toBeNull();
    expect(view.events).toEqual([{ type: "meta", bytes: total }]); // meta 只在头部就绪发一枚
    view.handle.destroy();
  });

  it("XSS 消毒(DOMPurify 生效):script/onerror/javascript: href 剔除,安全正文保留", async () => {
    const src = [
      "# 安全标题",
      "",
      "<script>window.__mtdss_xss = 1</script>",
      "",
      '<img src=x onerror="window.__mtdss_xss = 2">',
      "",
      "[点击](javascript:alert(1))",
      "",
      "**加粗** 与 `代码`",
      "",
    ].join("\n");
    const data = new TextEncoder().encode(src);
    sequentialFetch([bytesResponse(data, { begin: 0, total: data.length })]);

    const view = mount(textReq("md", { raw: RAW_URL }, "投毒.md"));
    await settleUntil(() => view.mdBody()?.querySelector("h1") !== null);
    const body = view.mdBody()!;
    expect(body.querySelector("script")).toBeNull();
    expect(body.querySelector("img[onerror]")).toBeNull();
    expect(body.querySelector('a[href^="javascript"]')).toBeNull();
    // 安全正文保留
    expect(body.querySelector("h1")?.textContent).toBe("安全标题");
    expect(body.querySelector("strong")?.textContent).toBe("加粗");
    expect(body.querySelector("code")?.textContent).toBe("代码");
    expect(body.textContent).toContain("点击");
    view.handle.destroy();
  });

  it("头部失败:覆盖层+重试钮+retry_needed;重试恢复渲染", async () => {
    const src = "# 恢复\n";
    const data = new TextEncoder().encode(src);
    const fetchMock = sequentialFetch([errBytesResponse(), bytesResponse(data, { begin: 0, total: data.length })]);

    const view = mount(textReq("md", { raw: RAW_URL }, "断流.md"));
    await settle();
    expect(view.overlay()!.classList.contains("mtdss-preview__overlay--hidden")).toBe(false);
    expect(view.retryButton()?.textContent).toBe(defaultPreviewStrings.retry);
    expect(view.events).toEqual([{ type: "retry_needed", message: defaultPreviewStrings.textLoadFailed }]);

    view.retryButton()!.click();
    await settleUntil(() => view.mdBody()?.querySelector("h1") !== null);
    expect(view.overlay()!.classList.contains("mtdss-preview__overlay--hidden")).toBe(true);
    expect(view.mdBody()?.querySelector("h1")?.textContent).toBe("恢复");
    expect(fetchMock).toHaveBeenCalledTimes(2);
    view.handle.destroy();
  });

  it("续读失败:覆盖层出+retry_needed;重试断点续读(起点不回退)", async () => {
    const head = headWindow("# 头部\n");
    const tail = new TextEncoder().encode("\n## 续段\n");
    const total = head.length + tail.length;
    const fetchMock = sequentialFetch([
      bytesResponse(head, { begin: 0, total }),
      errBytesResponse(),
      bytesResponse(tail, { begin: TEXT_HEAD_BYTES, total }),
    ]);

    const view = mount(textReq("md", { raw: RAW_URL }, "长文.md"));
    await settle();
    view.moreButton()!.click();
    await settle();
    expect(view.overlay()!.classList.contains("mtdss-preview__overlay--hidden")).toBe(false);
    expect(view.events[view.events.length - 1]).toEqual({ type: "retry_needed", message: defaultPreviewStrings.textLoadFailed });

    view.retryButton()!.click();
    await settleUntil(() => view.mdBody()?.querySelector("h2") !== null);
    expect(view.mdBody()?.querySelector("h2")?.textContent).toBe("续段");
    expect(view.moreButton()).toBeNull();
    // 断点:第 3 请求起点仍=头部窗末(失败不回退不跳读)
    expect(rangeHeaderOf(fetchMock.mock.calls[2] as unknown[])).toBe(
      `bytes=${TEXT_HEAD_BYTES}-${TEXT_HEAD_BYTES + TEXT_HEAD_BYTES - 1}`,
    );
    view.handle.destroy();
  });

  it("raw 缺席:终态 error 零 fetch;文案覆写穿透续读钮", async () => {
    const fetchMock = stubFetch({});
    const view = mount(textReq("md", {}, "空.md", { textLoadMore: "More" }));
    await settle();
    expect(view.events).toEqual([{ type: "error", message: defaultPreviewStrings.unavailableMessage }]);
    expect(fetchMock).not.toHaveBeenCalled();
    view.handle.destroy();
  });

  it("头部在途中 destroy:请求 abort、DOM 摘净、迟火零事件", async () => {
    const pending = deferred<Response>();
    const fetchMock = vi.fn((_url: unknown, _init?: RequestInit) => pending.promise);
    vi.stubGlobal("fetch", fetchMock);

    const view = mount(textReq("md", { raw: RAW_URL }, "慢.md"));
    await settle();
    view.handle.destroy();
    expect(fetchMock.mock.calls[0][1]?.signal?.aborted).toBe(true);
    expect(view.el.querySelector(".mtdss-preview")).toBeNull();

    pending.resolve(bytesResponse(headWindow("# 迟到\n"), { begin: 0, total: TEXT_HEAD_BYTES }));
    await settle();
    expect(view.events).toEqual([]);
  });
});

describe("text(代码/txt):语言猜定与纯文本回退", () => {
  it("扩展名猜语言:main.rs → rust 高亮(language-rust 标记 + hljs token)", async () => {
    const src = "fn main() {\n    let x = 1;\n}\n";
    const data = new TextEncoder().encode(src);
    sequentialFetch([bytesResponse(data, { begin: 0, total: data.length })]);

    const view = mount(textReq("text", { raw: RAW_URL }, "main.rs"));
    await settleUntil(() => (view.code()?.innerHTML ?? "").includes("hljs-"));
    const code = view.code()!;
    expect(code.className).toContain("language-rust");
    expect(code.innerHTML).toContain("hljs-");
    expect(code.textContent).toContain("fn main");
    expect(view.events).toEqual([{ type: "meta", bytes: data.length }]);
    view.handle.destroy();
  });

  it("猜不出语言:data.xyz → 纯文本 textContent(零高亮 token、零 HTML 解析)", async () => {
    const src = "a < b && c > d\n<script>alert(1)</script>";
    const data = new TextEncoder().encode(src);
    sequentialFetch([bytesResponse(data, { begin: 0, total: data.length })]);

    const view = mount(textReq("text", { raw: RAW_URL }, "data.xyz"));
    await settle();
    const code = view.code()!;
    expect(code.textContent).toBe(src); // 原词直出
    expect(code.querySelector("span")).toBeNull(); // 无高亮 token
    expect(code.innerHTML).not.toContain("<script>"); // 零 HTML 注入(textContent 面)
    view.handle.destroy();
  });

  it("无扩展名与点首隐藏文件均纯文本回退", async () => {
    const src = "plain content\n";
    const data = new TextEncoder().encode(src);
    sequentialFetch([bytesResponse(data, { begin: 0, total: data.length })]);

    const view = mount(textReq("text", { raw: RAW_URL }, "Makefile"));
    await settle();
    expect(view.code()?.className).not.toContain("language-");
    expect(view.code()?.querySelector("span")).toBeNull();
    view.handle.destroy();
  });
});
