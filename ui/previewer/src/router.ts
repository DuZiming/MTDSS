import { mountImage } from "./image";
import { ensureStyles } from "./styles";
import type { PreviewEvent, PreviewHandle, PreviewRequest } from "./types";

/// 类型路由:kind → 查看器。已实现 image;其余 kind 出占位面板
/// (「该格式暂未支持」+下载引导文案位;T17 pdf/text/md、T18 office 逐格式替换 case 臂)。
export function mountPreview(
  el: HTMLElement,
  req: PreviewRequest,
  onEvent: (e: PreviewEvent) => void,
): PreviewHandle {
  switch (req.kind) {
    case "image":
      return mountImage(el, req, onEvent);
    default:
      return mountPlaceholder(el, req, onEvent);
  }
}

/// 占位面板:文件名 + 「该格式暂未支持」 + 下载引导文案位。
/// 下载引导位:urls.raw 在 = 可执行下载链接(href=raw、download=原名);
/// raw 缺 = 静态引导文案。容器(T20/移动端)可按类名定位替换为自有动作。
function mountPlaceholder(
  el: HTMLElement,
  req: PreviewRequest,
  _onEvent: (e: PreviewEvent) => void,
): PreviewHandle {
  ensureStyles();
  const root = document.createElement("div");
  root.className = "mtdss-preview mtdss-preview--placeholder";

  const name = document.createElement("div");
  name.className = "mtdss-preview__placeholder-name";
  name.textContent = req.meta.name;

  const note = document.createElement("div");
  note.className = "mtdss-preview__placeholder-note";
  note.textContent = "该格式暂未支持";

  const download = document.createElement("div");
  download.className = "mtdss-preview__download";
  if (req.urls.raw !== undefined) {
    const link = document.createElement("a");
    link.className = "mtdss-preview__download-link";
    link.href = req.urls.raw;
    link.download = req.meta.name;
    link.textContent = "下载后查看";
    download.appendChild(link);
  } else {
    const hint = document.createElement("span");
    hint.className = "mtdss-preview__placeholder-note";
    hint.textContent = "可下载后查看";
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
