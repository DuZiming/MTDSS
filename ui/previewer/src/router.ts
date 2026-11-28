import { mountAudio } from "./audio";
import { mountImage } from "./image";
import { mountDocx, mountPptx, mountXlsx } from "./office";
import { mountPdf } from "./pdf";
import { mountText } from "./text";
import { mountUnsupported } from "./unsupported";
import { mountVideo } from "./video";
import { resolvePreviewStrings } from "./strings";
import type { PreviewStrings } from "./strings";
import type { PreviewEvent, PreviewHandle, PreviewRequest } from "./types";

/// 类型路由:kind → 查看器。image/pdf/md/text/docx/xlsx/pptx/video/audio
/// 已实现(T21 媒体接线);其余 kind 出占位面板(「该格式暂未支持」+下载
/// 引导文案位)。
///
/// 文案容器缝(T16-M1):req.strings(可缺)合并缺省中文成全量 strings,注入各查看器;
/// 占位面板同缝。桌面/移动容器后续自注入覆写。

export function mountPreview(
  el: HTMLElement,
  req: PreviewRequest,
  onEvent: (e: PreviewEvent) => void,
  nav?: { prev(): void; next(): void },
): PreviewHandle {
  const strings = resolvePreviewStrings(req.strings);
  switch (req.kind) {
    case "image":
      return mountImage(el, req, onEvent, strings, nav);
    case "pdf":
      return mountPdf(el, req, onEvent, strings);
    case "md":
    case "text":
      return mountText(el, req, onEvent, strings);
    case "docx":
      return mountDocx(el, req, onEvent, strings);
    case "xlsx":
      return mountXlsx(el, req, onEvent, strings);
    case "pptx":
      return mountPptx(el, req, onEvent, strings);
    case "video":
      return mountVideo(el, req, onEvent, strings);
    case "audio":
      return mountAudio(el, req, onEvent, strings);
    default:
      return mountPlaceholder(el, req, strings);
  }
}

/// 占位面板 = unsupported 形(router/office 降级共用单点,见 unsupported.ts)。
function mountPlaceholder(
  el: HTMLElement,
  req: PreviewRequest,
  strings: PreviewStrings,
): PreviewHandle {
  return mountUnsupported(el, req, strings);
}
