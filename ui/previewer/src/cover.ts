/// 静态封面挂载(field r13 #4,用户裁定「右侧栏预览只展示静态内容」):
/// 按 kind 出一帧不可交互的封面/图标——image/video=<img>(video 走服务端
/// 海报帧 thumb 面);audio/unsupported=内联 SVG 图标;text/md=前段文本
/// (Range 取头 4KiB);pdf=pdf.js 首页 canvas(URL 直连,http Range 续取,
/// 只渲一页);docx/xlsx/pptx=完整挂载的裁剪静态呈现(尺寸门 ≤25MiB,
/// 超门/失败=文件图标)。零事件零回调,destroy 摘净。
///
/// 与 mountPreview 的分工:预览主体=⧉ 弹窗(交互查看器);面板=静态封面
/// +元数据(本模块)。CSP 同族纪律:图标内联 SVG 零外联。
import { renderMarkdownHtml } from "./text";
import { mountPreview } from "./router";
import { coverGet, coverPut } from "./cover-cache";
import { pdfWorkerSrc } from "./pdf-worker";
import { pdfWasmParams } from "./pdf-wasm";
import type { PreviewRequest } from "./types";

/// office 封面尺寸门(裁剪呈现需整文件下载;超门落图标,大文件面板
/// 一瞥不付整档流量——office 流式票在池,落地后此门可撤)。
export const COVER_OFFICE_MAX_BYTES = 25 * 1024 * 1024;

/// 文本封面取头字节窗(Range: bytes=0-4095)。
const TEXT_SNIPPET_BYTES = 4096;

export interface CoverHandle {
  destroy(): void;
}

/// 内联 SVG 图标(24×24 视窗,描边风格与主壳图标族近似;零外联)。
const ICONS = {
  file:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/></svg>',
  audio:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9 18V6l10-2v12"/><circle cx="6" cy="18" r="3"/><circle cx="19" cy="16" r="3"/></svg>',
  video:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="6" width="13" height="12" rx="2"/><path d="m16 10 5-3v10l-5-3"/></svg>',
} as const;

function iconEl(kind: keyof typeof ICONS): HTMLElement {
  const el = document.createElement("div");
  el.className = "mtdss-cover__icon";
  el.innerHTML = ICONS[kind];
  return el;
}

export function mountCover(el: HTMLElement, req: PreviewRequest): CoverHandle {
  let destroyed = false;
  const root = document.createElement("div");
  root.className = "mtdss-cover";
  el.appendChild(root);

  /// 迟到守卫:异步面(文本/pdf/office)destroy 后不再落 DOM。
  const alive = (): boolean => !destroyed;

  /// 尽力而为替换:失败/迟到 → 文件图标兜底。
  const fallback = (): void => {
    if (!alive() || root.childElementCount > 0) return;
    root.appendChild(iconEl("file"));
  };

  const mountImg = (url: string, alt: string, failIcon: keyof typeof ICONS): void => {
    const img = document.createElement("img");
    img.className = "mtdss-cover__img";
    img.alt = alt;
    img.draggable = false;
    img.addEventListener("error", () => {
      if (!alive()) return;
      root.textContent = "";
      root.appendChild(iconEl(failIcon));
    });
    img.src = url;
    root.appendChild(img);
  };

  switch (req.kind) {
    case "image":
      mountImg(req.urls.thumb ?? req.urls.raw ?? "", req.meta.name, "file");
      break;
    case "video":
      // 服务端海报帧 thumb 面(T15-M7);404/无帧 → 视频图标
      mountImg(req.urls.thumb ?? "", req.meta.name, "video");
      break;
    case "audio":
      root.appendChild(iconEl("audio"));
      break;
    case "md":
    case "text": {
      // 前段文本:Range 取头窗。md(field r16 #2:面板也渲染格式,不再
      // 裸 markdown 源文)走 markdown-it+消毒管线取渲染后首屏;纯文本
      // 渲染为静态片段(前 ~24 行)。
      const url = req.urls.raw;
      if (url === undefined) {
        fallback();
        break;
      }
      void (async () => {
        try {
          const res = await fetch(url, { headers: { Range: `bytes=0-${TEXT_SNIPPET_BYTES - 1}` } });
          if (!res.ok) throw new Error(`HTTP ${res.status}`);
          const buf = await res.arrayBuffer();
          if (!alive()) return;
          // BOM 嗅探(field r14 #2 修「ini 显示成 16 进制」:Windows 记事本
          // 系常存 UTF-16LE,按 UTF-8 误解即满屏乱码):FF FE→utf-16le、
          // FE FF→utf-16be、EF BB BF→剥 BOM 的 utf-8;无 BOM 落 utf-8。
          const bytes = new Uint8Array(buf);
          let text: string;
          if (bytes[0] === 0xff && bytes[1] === 0xfe) {
            text = new TextDecoder("utf-16le").decode(buf);
          } else if (bytes[0] === 0xfe && bytes[1] === 0xff) {
            text = new TextDecoder("utf-16be").decode(buf);
          } else if (bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) {
            text = new TextDecoder("utf-8").decode(buf.slice(3));
          } else {
            text = new TextDecoder("utf-8", { fatal: false }).decode(buf);
          }
          text = text.replace(/^﻿/, "");
          if (req.kind === "md") {
            // 面板 md 封面=渲染格式(r16 #2):markdown-it+DOMPurify 与
            // 弹窗查看器同管线(text.ts 单例门)。
            const html = await renderMarkdownHtml(text);
            if (!alive()) return;
            const box = document.createElement("div");
            box.className = "mtdss-cover__md";
            box.innerHTML = html;
            root.appendChild(box);
            return;
          }
          const pre = document.createElement("pre");
          pre.className = "mtdss-cover__pre";
          pre.textContent = text.split(/\r?\n/).slice(0, 24).join("\n");
          root.appendChild(pre);
        } catch {
          fallback();
        }
      })();
      break;
    }
    case "pdf": {
      // pdf.js 首页 canvas(URL 直连,http Range 续取只拉首页所需字节)
      const url = req.urls.raw;
      if (url === undefined) {
        fallback();
        break;
      }
      void (async () => {
        try {
          // r70e legacy 构建(Chromium 83 实测面;同 pdf.ts 注记)
          const pdfjs = await import("pdfjs-dist/legacy/build/pdf.mjs");
          if (!alive()) return;
          pdfjs.GlobalWorkerOptions.workerSrc = pdfWorkerSrc();
          // r45 持久封面缓存:键在场先取(跨会话命中零重渲);miss 现渲
          // 后回填。键缺席(boot 未携)跳过,行为=现状。
          const cacheKey = req.meta.key;
          if (cacheKey !== undefined) {
            const hit = await coverGet(`pdf-cover|${cacheKey}`);
            if (!alive()) return;
            if (hit !== null) {
              const img = document.createElement("img");
              img.className = "mtdss-cover__canvas";
              img.src = hit;
              root.appendChild(img);
              return;
            }
          }
          const doc = await pdfjs.getDocument({ url, ...pdfWasmParams() }).promise;
          if (!alive()) return;
          const page = await doc.getPage(1);
          if (!alive()) return;
          const base = page.getViewport({ scale: 1 });
          const width = Math.max(120, root.clientWidth || 240);
          const scale = Math.min(2, width / base.width);
          const viewport = page.getViewport({ scale });
          const canvas = document.createElement("canvas");
          canvas.className = "mtdss-cover__canvas";
          canvas.width = Math.floor(viewport.width);
          canvas.height = Math.floor(viewport.height);
          await page.render({ canvas, viewport }).promise;
          if (!alive()) return;
          // r45:渲毕回填持久缓存(toDataURL 与命中面同构)
          if (cacheKey !== undefined) {
            try {
              await coverPut(`pdf-cover|${cacheKey}`, canvas.toDataURL("image/jpeg", 0.85));
            } catch {
              /* 回填失败静默:缓存层不阻塞渲染路径 */
            }
          }
          root.appendChild(canvas);
        } catch {
          fallback();
        }
      })();
      break;
    }
    case "docx":
    case "xlsx":
    case "pptx": {
      // 裁剪静态呈现:完整挂载装进不可交互裁剪壳(首页面板顶视);
      // 尺寸门内才做(整文件下载),超门/失败=文件图标
      if (req.meta.size > COVER_OFFICE_MAX_BYTES || req.urls.raw === undefined) {
        root.appendChild(iconEl("file"));
        break;
      }
      const clip = document.createElement("div");
      clip.className = "mtdss-cover__clip";
      root.appendChild(clip);
      const inner = mountPreview(clip, req, () => {});
      return {
        destroy: () => {
          if (destroyed) return;
          destroyed = true;
          inner.destroy();
          root.remove();
        },
      };
    }
    // eslint-disable-next-line no-fall-through
    default:
      root.appendChild(iconEl("file"));
      break;
  }

  return {
    destroy: () => {
      if (destroyed) return;
      destroyed = true;
      root.remove();
    },
  };
}
