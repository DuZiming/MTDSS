// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, afterEach } from "vitest";

import { mountPreview } from "./router";
import { stubFetch, unstubAll, okResponse, blob, RAW_URL } from "./testutil";
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
  it("kind=image → 图片查看器装配(img 元素在,非占位文案)", () => {
    stubFetch({ [RAW_URL]: okResponse(blob(4)) });
    const el = document.createElement("div");
    document.body.appendChild(el);
    const handle = mountPreview(el, req("image", { raw: RAW_URL }, "图.jpg"), () => {});
    expect(el.querySelector("img.mtdss-preview__img")).not.toBeNull();
    expect(textOf(el)).not.toContain("该格式暂未支持");
    expect(typeof handle.destroy).toBe("function");
    handle.destroy();
  });

  it("未实现 kind(pdf/video/audio/docx/xlsx/pptx/md/text/unsupported)→ 占位面板:该格式暂未支持+文件名", () => {
    for (const kind of ["pdf", "video", "audio", "docx", "xlsx", "pptx", "md", "text", "unsupported"] as const) {
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
    const a = withRaw.querySelector<HTMLAnchorElement>("a.mtdss-preview__download-link");
    expect(a, "raw 在 → 应出下载链接").not.toBeNull();
    expect(a!.href).toBe(RAW_URL);
    expect(a!.download).toBe("样本.pdf");
    h1.destroy();

    const noRaw = document.createElement("div");
    document.body.appendChild(noRaw);
    const h2 = mountPreview(noRaw, req("pdf", {}), () => {});
    expect(noRaw.querySelector("a")).toBeNull();
    expect(textOf(noRaw)).toContain("下载");
    h2.destroy();
  });

  it("占位 destroy → 装配 DOM 摘净;样式标签留驻(文档级单例,幂等不重复注入)", () => {
    const el = document.createElement("div");
    document.body.appendChild(el);
    const handle = mountPreview(el, req("md", {}), () => {});
    expect(document.querySelectorAll("#mtdss-preview-styles").length).toBe(1);
    handle.destroy();
    expect(el.querySelector(".mtdss-preview")).toBeNull();
    // 二次挂载不重复注入样式
    const el2 = document.createElement("div");
    document.body.appendChild(el2);
    mountPreview(el2, req("md", {}), () => {}).destroy();
    expect(document.querySelectorAll("#mtdss-preview-styles").length).toBe(1);
    el2.querySelector(".mtdss-preview")?.remove();
  });

  it("样式 scoped:只挂 .mtdss-preview 前缀类,无全局选择器污染", () => {
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
  it("事件面只出 error/retry_needed/meta 三型,分发不改道", () => {
    stubFetch({ [RAW_URL]: okResponse(blob(4)) });
    const el = document.createElement("div");
    document.body.appendChild(el);
    const events: PreviewEvent[] = [];
    const handle = mountPreview(el, req("image", { raw: RAW_URL }), (e) => void events.push(e));
    handle.destroy();
    for (const e of events) {
      expect(["error", "retry_needed", "meta"]).toContain(e.type);
    }
  });
});
