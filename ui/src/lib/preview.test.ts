/// 预览装配单点(V4-1 T20)单测:kind 扩展名映射表(URL 组装 `?t=` 查询
/// 令牌 + jpeg 面只随 HEIC + meta)+ 回环 URL 形(brief 钉死
/// `http://127.0.0.1:{port}/f/{handle}/{face}?t={token}`,thumb 另携 w=)。
/// 依赖面:preview.ts 仅类型引用(previewer 契约 + daemon PreviewOpenInfo),
/// 运行时零 import,直驱纯函数。
import { describe, expect, it } from "vitest";

import type { PreviewOpenInfo } from "./daemon";
import {
  PREVIEW_THUMB_W,
  buildPreviewRequest,
  isHeic,
  previewKindOf,
  previewLoopbackUrl,
} from "./preview";

const INFO: PreviewOpenInfo = { handle: "h-1", port: 45678, token: "tok123" };

describe("previewKindOf(扩展名映射表,报告钉面)", () => {
  it("image 词表(filetype img 同源)", () => {
    for (const n of ["a.JPG", "b.jpeg", "c.png", "d.gif", "e.bmp", "f.webp", "g.heic", "h.svg", "i.tif", "j.tiff", "k.ico", "l.avif"]) {
      expect(previewKindOf(n), n).toBe("image");
    }
  });

  it("pdf/office/md", () => {
    expect(previewKindOf("说明书.pdf")).toBe("pdf");
    expect(previewKindOf("合同_v3.docx")).toBe("docx");
    expect(previewKindOf("老档.doc")).toBe("docx");
    expect(previewKindOf("年度汇总.xlsx")).toBe("xlsx");
    expect(previewKindOf("表格.xls")).toBe("xlsx");
    expect(previewKindOf("数据.csv")).toBe("xlsx");
    expect(previewKindOf("汇报.pptx")).toBe("pptx");
    expect(previewKindOf("旧稿.ppt")).toBe("pptx");
    expect(previewKindOf("笔记.md")).toBe("md");
  });

  it("video/audio 词表(T21 前占位)", () => {
    for (const n of ["a.mp4", "b.mkv", "c.mov", "d.avi", "e.webm", "f.m4v", "g.wmv", "h.flv", "i.mpg", "j.mpeg", "k.3gp"]) {
      expect(previewKindOf(n), n).toBe("video");
    }
    for (const n of ["a.mp3", "b.flac", "c.wav", "d.aac", "e.ogg", "f.m4a", "g.wma", "h.opus"]) {
      expect(previewKindOf(n), n).toBe("audio");
    }
  });

  it("text:txt/log + 常见代码/数据扩展", () => {
    for (const n of ["a.txt", "b.log", "c.rs", "d.ts", "e.py", "f.json", "g.yml", "h.xml", "i.html", "j.css", "k.sql", "l.sh"]) {
      expect(previewKindOf(n), n).toBe("text");
    }
  });

  it("无点/点文件/点尾/未知扩展名 → unsupported(previewer 占位面板)", () => {
    expect(previewKindOf("无扩展名")).toBe("unsupported");
    expect(previewKindOf(".gitignore")).toBe("unsupported");
    expect(previewKindOf("archive.zip")).toBe("unsupported");
    expect(previewKindOf("安装包.exe")).toBe("unsupported");
  });
});

describe("previewLoopbackUrl(URL 组装单点,?t= 查询令牌)", () => {
  it("raw 面携 t=,不含 w", () => {
    expect(previewLoopbackUrl(INFO, "raw")).toBe(
      "http://127.0.0.1:45678/f/h-1/raw?t=tok123",
    );
  });

  it("jpeg 面携 t=(HEIC 端点)", () => {
    expect(previewLoopbackUrl(INFO, "jpeg")).toBe(
      "http://127.0.0.1:45678/f/h-1/jpeg?t=tok123",
    );
  });

  it("thumb 面携 t= 与 w=", () => {
    expect(previewLoopbackUrl(INFO, "thumb", PREVIEW_THUMB_W)).toBe(
      `http://127.0.0.1:45678/f/h-1/thumb?t=tok123&w=${PREVIEW_THUMB_W}`,
    );
    expect(PREVIEW_THUMB_W).toBe(256);
  });
});

describe("buildPreviewRequest(kind 路由 + urls 装配)", () => {
  it("图片:raw+thumb(w=256),meta 名/尺寸透传", () => {
    const req = buildPreviewRequest(INFO, "照片.jpg", 1234);
    expect(req.kind).toBe("image");
    expect(req.urls.raw).toBe("http://127.0.0.1:45678/f/h-1/raw?t=tok123");
    expect(req.urls.thumb).toBe(
      "http://127.0.0.1:45678/f/h-1/thumb?t=tok123&w=256",
    );
    expect(req.urls.jpeg).toBeUndefined();
    expect(req.meta).toEqual({ name: "照片.jpg", size: 1234 });
  });

  it("HEIC:image 才开 jpeg 面(非 HEIC 端点恒 404,只随 heic 装配)", () => {
    const req = buildPreviewRequest(INFO, "IMG_0001.heic", 10);
    expect(req.urls.jpeg).toBe("http://127.0.0.1:45678/f/h-1/jpeg?t=tok123");
    expect(buildPreviewRequest(INFO, "IMG_0001.jpg", 10).urls.jpeg).toBeUndefined();
  });

  it("非图 kind:仅 raw(pdf 无渐进面,占位面板下载引导同走)", () => {
    const req = buildPreviewRequest(INFO, "说明书.pdf", 5);
    expect(req.kind).toBe("pdf");
    expect(req.urls.raw).toBeDefined();
    expect(req.urls.thumb).toBeUndefined();
    expect(req.urls.jpeg).toBeUndefined();
  });

  it("unsupported 占位:仅 raw(占位面板出下载链接)", () => {
    const req = buildPreviewRequest(INFO, "包.zip", 7);
    expect(req.kind).toBe("unsupported");
    expect(req.urls.raw).toBeDefined();
  });

  it("video:raw+hls(T15 播放列表面)+thumb 海报帧(T21 接线;audio:仅 raw 直连)", () => {
    const v = buildPreviewRequest(INFO, "影片.mp4", 9);
    expect(v.kind).toBe("video");
    expect(v.urls.hls).toBe("http://127.0.0.1:45678/f/h-1/video/index.m3u8?t=tok123");
    expect(v.urls.thumb).toBe("http://127.0.0.1:45678/f/h-1/thumb?t=tok123&w=256");
    expect(v.urls.jpeg).toBeUndefined();
    const a = buildPreviewRequest(INFO, "曲目.mp3", 8);
    expect(a.kind).toBe("audio");
    expect(a.urls.raw).toBeDefined();
    expect(a.urls.hls).toBeUndefined();
    expect(a.urls.thumb).toBeUndefined();
  });
});

describe("isHeic", () => {
  it("heic 大小写不敏感;非 heic false", () => {
    expect(isHeic("a.heic")).toBe(true);
    expect(isHeic("A.HEIC")).toBe(true);
    expect(isHeic("b.jpg")).toBe(false);
    expect(isHeic("heic")).toBe(false);
  });
});
