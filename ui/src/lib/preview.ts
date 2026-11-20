/// 预览装配单点(V4-1 T20):回环口 URL 组装 + 文件 kind 判定 +
/// PreviewRequest 装配(面板与弹出独立窗共用同一份;brief「URL 组装单点
/// 模块(可测)」)。
///
/// URL 形(brief 钉死):`http://127.0.0.1:{port}/f/{handle}/{face}?t={token}`
/// —face ∈ raw|jpeg|thumb;thumb 另携 `&w=`。鉴权走查询串 `?t=`(V4-1 T20
/// 控制器裁定②,previewer bundle 内 fetch 不带头、`<img>`/`<video>` 直连
/// 更无法携头;EntryThumb 的 Bearer 头缝不动,零回归)。
///
/// kind 判定沿 previewer types(T16 契约十值),扩展名映射表(小写,报告
/// 同表钉面):
/// - image: jpg jpeg png gif bmp webp heic svg tif tiff ico avif(filetype
///   img 词表同源);仅 heic 另开 jpeg 面(HEIC 转码端点,非 HEIC 恒 404,
///   image 查看器 urls.jpeg 优先于 raw——误装即全量图片落重试面);
/// - pdf: pdf;
/// - video: mp4 mkv mov avi webm m4v wmv flv mpg mpeg 3gp(filetype vid
///   词表同源;T21 前占位面板);
/// - audio: mp3 flac wav aac ogg m4a wma opus(filetype 词表;T21 前占位);
/// - docx: doc docx;xlsx: xls xlsx csv;pptx: ppt pptx;
/// - md: md;
/// - text: txt log + 常见代码/数据扩展(text.ts 高亮语言表同域);
/// - 其余(无扩展名/点文件/压缩包/可执行等)→ unsupported(previewer
///   占位面板,raw 在场出「下载后查看」链接)。
///
/// raw 恒装配:全 kind 的占位降级面/下载引导都走它(office 帽面由
/// previewer 内部按 meta.size 自门,T18)。运行时零 import(仅类型引用),
/// 测试直驱纯函数。

import type { PreviewKind, PreviewRequest } from "@mtdss/previewer";
import type { PreviewOpenInfo } from "./daemon";

/// thumb 请求宽(brief 钉死 256;渐进首显面,列表 EntryThumb 各档另有宽)。
export const PREVIEW_THUMB_W = 256;

/// 扩展名 → PreviewKind 映射表(见模块注;小写键,匹配同归小写)。
const KIND_BY_EXT: Readonly<Record<string, PreviewKind>> = {
  // image(filetype img 词表同源)
  jpg: "image",
  jpeg: "image",
  png: "image",
  gif: "image",
  bmp: "image",
  webp: "image",
  heic: "image",
  svg: "image",
  tif: "image",
  tiff: "image",
  ico: "image",
  avif: "image",
  pdf: "pdf",
  // video(filetype vid 词表同源;T21 前占位)
  mp4: "video",
  mkv: "video",
  mov: "video",
  avi: "video",
  webm: "video",
  m4v: "video",
  wmv: "video",
  flv: "video",
  mpg: "video",
  mpeg: "video",
  "3gp": "video",
  // audio(filetype 词表;T21 前占位)
  mp3: "audio",
  flac: "audio",
  wav: "audio",
  aac: "audio",
  ogg: "audio",
  m4a: "audio",
  wma: "audio",
  opus: "audio",
  // office
  doc: "docx",
  docx: "docx",
  xls: "xlsx",
  xlsx: "xlsx",
  csv: "xlsx",
  ppt: "pptx",
  pptx: "pptx",
  md: "md",
  // text(txt/log + 常见代码/数据扩展)
  txt: "text",
  log: "text",
  rs: "text",
  ts: "text",
  tsx: "text",
  js: "text",
  mjs: "text",
  cjs: "text",
  jsx: "text",
  py: "text",
  go: "text",
  java: "text",
  kt: "text",
  kts: "text",
  swift: "text",
  c: "text",
  h: "text",
  cpp: "text",
  cc: "text",
  cxx: "text",
  hpp: "text",
  cs: "text",
  rb: "text",
  php: "text",
  sh: "text",
  bash: "text",
  zsh: "text",
  ps1: "text",
  bat: "text",
  json: "text",
  jsonc: "text",
  yml: "text",
  yaml: "text",
  toml: "text",
  ini: "text",
  cfg: "text",
  conf: "text",
  xml: "text",
  html: "text",
  css: "text",
  sql: "text",
};

/// 取小写扩展名:最后一个点之后;无点、点在首位(点文件)或点在末尾
/// → null(filetype.extOf 同口径)。
function extOf(name: string): string | null {
  const i = name.lastIndexOf(".");
  if (i <= 0 || i === name.length - 1) return null;
  return name.slice(i + 1).toLowerCase();
}

/// 文件名 → previewer kind(未知扩展名 → unsupported 占位)。
export function previewKindOf(name: string): PreviewKind {
  const ext = extOf(name);
  return (ext != null && KIND_BY_EXT[ext]) || "unsupported";
}

/// 是否 HEIC 源(jpeg 面=HEIC 端点,非 HEIC 恒 404 → 只对 heic 装配)。
export function isHeic(name: string): boolean {
  return extOf(name) === "heic";
}

/// 回环 URL 组装单点:`http://127.0.0.1:{port}/f/{handle}/{face}?t={token}`
/// (thumb 另携 `&w=`;`w` undefined = 不携,face 非 thumb 恒不携)。
export function previewLoopbackUrl(
  info: PreviewOpenInfo,
  face: "raw" | "jpeg" | "thumb",
  w?: number,
): string {
  let url = `http://127.0.0.1:${info.port}/f/${info.handle}/${face}?t=${info.token}`;
  if (w !== undefined) url += `&w=${w}`;
  return url;
}

/// PreviewRequest 装配(面板挂载与 ⧉ 弹出独立窗共用同一份):kind 路由 +
/// urls(raw 恒携;image 才携 thumb 渐进面;仅 heic 才携 jpeg 转码面)+
/// meta(名/尺寸,office 帽面与面板文案数据源)。
export function buildPreviewRequest(
  info: PreviewOpenInfo,
  name: string,
  size: number,
): PreviewRequest {
  const kind = previewKindOf(name);
  const urls: PreviewRequest["urls"] = { raw: previewLoopbackUrl(info, "raw") };
  if (kind === "image") {
    urls.thumb = previewLoopbackUrl(info, "thumb", PREVIEW_THUMB_W);
    if (isHeic(name)) urls.jpeg = previewLoopbackUrl(info, "jpeg");
  }
  return { kind, urls, meta: { name, size } };
}
