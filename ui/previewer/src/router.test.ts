// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { fakePdfjsState, resetFakePdfjs } from "./pdfjs-fake";

vi.mock("pdfjs-dist", async () => {
  const fake = await import("./pdfjs-fake");
  return fake.makeFakePdfjs();
});
vi.mock("pdfjs-dist/build/pdf.worker.min.mjs", () => ({}));

import { mountPreview } from "./router";
import { bytesResponse, okResponse, blob, RAW_URL, sequentialFetch, settle, settleUntil, stubFetch, unstubAll } from "./testutil";
import type { PreviewEvent, PreviewRequest } from "./types";

function req(kind: PreviewRequest["kind"], urls: PreviewRequest["urls"] = {}, name = "样本.pdf"): PreviewRequest {
  return { kind, urls, meta: { name, size: 1234 } };
}

function textOf(el: HTMLElement): string {
  return (el.textContent ?? "").trim();
}

beforeEach(() => {
  document.body.innerHTML = "";
  document.getElementById("mtdss-preview-styles")?.remove();
});

afterEach(() => {
  unstubAll();
});

describe("mountPreview kind 路由分发", () => {
  it("kind=image → 图片查看器装配(img 元素在,非占位文案)", async () => {
    stubFetch({ [RAW_URL]: okResponse(blob(4)) });
    const el = document.createElement("div");
    document.body.appendChild(el);
    const handle = mountPreview(el, req("image", { raw: RAW_URL }, "图.jpg"), () => {});
    await settle();
    expect(el.querySelector("img.mtdss-preview__img")).not.toBeNull();
    expect(textOf(el)).not.toContain("该格式暂未支持");
    expect(typeof handle.destroy).toBe("function");
    handle.destroy();
  });

  it("kind=pdf → pdf 查看器装配(--pdf 根,非占位文案;文档就绪出画布)", async () => {
    resetFakePdfjs(3);
    const data = new Uint8Array(2048);
    sequentialFetch([bytesResponse(data, { begin: 0, total: data.length })]);
    const el = document.createElement("div");
    document.body.appendChild(el);
    const handle = mountPreview(el, req("pdf", { raw: RAW_URL }, "样本.pdf"), () => {});
    expect(textOf(el)).not.toContain("该格式暂未支持");
    await settle();
    expect(el.querySelector(".mtdss-preview--pdf")).not.toBeNull();
    expect(el.querySelectorAll("canvas.mtdss-preview__pdf-canvas").length).toBe(3);
    handle.destroy();
  });

  it("kind=md → md 渲染面(.md 容器在,非占位);kind=text → 代码面板(pre>code)", async () => {
    // 面注记:happy-dom 下 DOMPurify 产物损坏(h1 被剥),内容级断言归 text.test.ts(jsdom);
    // 此处只钉分发形状(容器元素装配 + 非占位文案)。
    const mdSrc = new TextEncoder().encode("# 路由\n");
    sequentialFetch([
      bytesResponse(mdSrc, { begin: 0, total: mdSrc.length }),
      bytesResponse(new TextEncoder().encode("let x = 1;\n"), { begin: 0, total: 10 }),
    ]);
    const elMd = document.createElement("div");
    document.body.appendChild(elMd);
    const h1 = mountPreview(elMd, req("md", { raw: RAW_URL }, "笔记.md"), () => {});
    await settleUntil(() => elMd.querySelector(".mtdss-preview__md") !== null);
    expect(elMd.querySelector(".mtdss-preview__md")).not.toBeNull();
    expect(textOf(elMd)).not.toContain("该格式暂未支持");
    h1.destroy();

    const elText = document.createElement("div");
    document.body.appendChild(elText);
    const h2 = mountPreview(elText, req("text", { raw: RAW_URL }, "脚本.js"), () => {});
    expect(elText.querySelector("pre .mtdss-preview__code-code")).not.toBeNull();
    expect(textOf(elText)).not.toContain("该格式暂未支持");
    h2.destroy();
  });

  it("未实现 kind(video/audio/unsupported)→ 占位面板:该格式暂未支持+文件名(docx/xlsx/pptx 自 T18 起走 office 查看器,分发面归 office.test.ts)", () => {
    for (const kind of ["video", "audio", "unsupported"] as const) {
      const el = document.createElement("div");
      document.body.appendChild(el);
      const handle = mountPreview(el, req(kind, {}, `文件.${kind}`), () => {});
      expect(textOf(el), `kind=${kind} 应出占位文案`).toContain("该格式暂未支持");
      expect(textOf(el)).toContain(`文件.${kind}`);
      expect(el.querySelector("img")).toBeNull();
      handle.destroy();
    }
  });

  it("占位含下载引导文案位:urls.raw 在 → 下载链接(href=raw、download=原名);raw 缺 → 静态引导文案", () => {
    const withRaw = document.createElement("div");
    document.body.appendChild(withRaw);
    const h1 = mountPreview(withRaw, req("pdf", { raw: RAW_URL }), () => {});
    // pdf 有真查看器:下载引导位属占位面板,换用 video 验证
    h1.destroy();
    const vRaw = document.createElement("div");
    document.body.appendChild(vRaw);
    const hv = mountPreview(vRaw, req("video", { raw: RAW_URL }), () => {});
    const a = vRaw.querySelector<HTMLAnchorElement>("a.mtdss-preview__download-link");
    expect(a, "raw 在 → 应出下载链接").not.toBeNull();
    expect(a!.href).toBe(RAW_URL);
    expect(a!.download).toBe("样本.pdf");
    hv.destroy();

    const noRaw = document.createElement("div");
    document.body.appendChild(noRaw);
    const h2 = mountPreview(noRaw, req("video", {}), () => {});
    expect(noRaw.querySelector("a")).toBeNull();
    expect(textOf(noRaw)).toContain("下载");
    h2.destroy();
  });

  it("占位 destroy → 装配 DOM 摘净;样式标签留驻(文档级单例,幂等不重复注入)", () => {
    const el = document.createElement("div");
    document.body.appendChild(el);
    const handle = mountPreview(el, req("video", {}), () => {});
    expect(document.querySelectorAll("#mtdss-preview-styles").length).toBe(1);
    handle.destroy();
    expect(el.querySelector(".mtdss-preview")).toBeNull();
    // 二次挂载不重复注入样式
    const el2 = document.createElement("div");
    document.body.appendChild(el2);
    mountPreview(el2, req("video", {}), () => {}).destroy();
    expect(document.querySelectorAll("#mtdss-preview-styles").length).toBe(1);
    el2.querySelector(".mtdss-preview")?.remove();
  });

  it("样式 scoped:只挂 .mtdss-preview 前缀类,无全局选择器污染", async () => {
    stubFetch({ [RAW_URL]: okResponse(blob(4)) });
    const el = document.createElement("div");
    document.body.appendChild(el);
    const handle = mountPreview(el, req("image", { raw: RAW_URL }, "图.jpg"), () => {});
    handle.destroy();
    const css = document.getElementById("mtdss-preview-styles")?.textContent ?? "";
    expect(css).toContain(".mtdss-preview");
    // 所有选择器都以 .mtdss-preview(或其局部变量/元素内作用)开头,不许裸元素选择器
    const selectors = css
      .split("}")
      .map((part) => part.split("{")[0]?.trim() ?? "")
      .filter((sel) => sel.length > 0 && !sel.startsWith("@"));
    for (const sel of selectors) {
      expect(sel.startsWith(".mtdss-preview"), `选择器必须 scoped:${sel}`).toBe(true);
    }
  });
});

describe("mountPreview 事件契约", () => {
  it("事件面只出 error/retry_needed/meta 三型,分发不改道(pdf/meta 扩展 page/pages 不破型)", async () => {
    resetFakePdfjs(3);
    const data = new Uint8Array(2048);
    stubFetch({ [RAW_URL]: bytesResponse(data, { begin: 0, total: data.length }) });
    const el = document.createElement("div");
    document.body.appendChild(el);
    const events: PreviewEvent[] = [];
    const handle = mountPreview(el, req("pdf", { raw: RAW_URL }), (e) => void events.push(e));
    await settle();
    handle.destroy();
    expect(events.length).toBeGreaterThan(0);
    for (const e of events) {
      expect(["error", "retry_needed", "meta"]).toContain(e.type);
    }
    // pdf 文档级接线已发生(getDocument 收到 range transport)
    expect(fakePdfjsState().docParams.length).toBe(1);
  });
});
