/// 占位面板:格式未支持/不可预览的统一形(文件名 +「该格式暂未支持」+ 下载引导文案位)。
/// router(未实现 kind)与 office 查看器(解析/渲染失败降级,T18)共用此单点——文案全走
/// 缝(unsupportedNote/downloadToView/downloadHint),两处改词一处生效。
///
/// 下载引导位:urls.raw 在 = 可执行下载链接(href=raw、download=原名);raw 缺 = 静态引导
/// 文案。容器(T20/移动端)可按类名定位替换为自有动作。

import type { PreviewStrings } from "./strings";
import type { PreviewHandle, PreviewRequest } from "./types";

export function mountUnsupported(
  el: HTMLElement,
  req: PreviewRequest,
  strings: PreviewStrings,
): PreviewHandle {
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
