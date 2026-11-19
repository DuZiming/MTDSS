import { mountImage } from "./image";
import { mountPdf } from "./pdf";
import { mountText } from "./text";
import { resolvePreviewStrings } from "./strings";
import { ensureStyles } from "./styles";
import type { PreviewStrings } from "./strings";
import type { PreviewEvent, PreviewHandle, PreviewRequest } from "./types";

/// 类型路由:kind → 查看器。image/pdf/md/text 已实现;其余 kind 出占位面板
/// (「该格式暂未支持」+下载引导文案位;T18 office 逐格式替换 case 臂)。
///
/// 文案容器缝(T16-M1):req.strings(可缺)合并缺省中文成全量 strings,注入各查看器;
/// 占位面板同缝。桌面/移动容器后续自注入覆写。

export function mountPreview(
  el: HTMLElement,
  req: PreviewRequest,
  onEvent: (e: PreviewEvent) => void,
): PreviewHandle {
  const strings = resolvePreviewStrings(req.strings);
  switch (req.kind) {
    case "image":
      return mountImage(el, req, onEvent, strings);
    case "pdf":
      return mountPdf(el, req, onEvent, strings);
    case "md":
    case "text":
      return mountText(el, req, onEvent, strings);
    default:
      return mountPlaceholder(el, req, strings);
  }
}

/// 占位面板:文件名 + 「该格式暂未支持」 + 下载引导文案位。
/// 下载引导位:urls.raw 在 = 可执行下载链接(href=raw、download=原名);
/// raw 缺 = 静态引导文案。容器(T20/移动端)可按类名定位替换为自有动作。
function mountPlaceholder(
  el: HTMLElement,
  req: PreviewRequest,
  strings: PreviewStrings,
): PreviewHandle {
  ensureStyles();
  const root = document.createElement("div");
  root.className = "mtdss-preview mtdss-preview--placeholder";

  const name = document.createElement("div");
  name.className = "mtdss-preview__placeholder-name";
  name.textContent = req.meta.name;

  const note = document.createElement("div");
  note.className = "mtdss-preview__placeholder-note";
  note.textContent = strings.unsupportedNote;

  const download = document.createElement("div");
  download.className = "mtdss-preview__download";
  if (req.urls.raw !== undefined) {
    const link = document.createElement("a");
    link.className = "mtdss-preview__download-link";
    link.href = req.urls.raw;
    link.download = req.meta.name;
    link.textContent = strings.downloadToView;
    download.appendChild(link);
  } else {
    const hint = document.createElement("span");
    hint.className = "mtdss-preview__placeholder-note";
    hint.textContent = strings.downloadHint;
    download.appendChild(hint);
  }

  root.append(name, note, download);
  el.appendChild(root);
  return {
    destroy: () => {
      root.remove();
    },
  };
}
