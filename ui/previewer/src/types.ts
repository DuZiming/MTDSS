/// 预览包公共契约(计划钉死,逐字):mountPreview + PreviewRequest + PreviewEvent。
/// urls 由容器装配(回环口 URL,鉴权由 daemon 回环中间件承担),包内原样消费不解析。

import type { PreviewStrings } from "./strings";

/// 预览格式类别:kind 分发唯一依据。
export type PreviewKind =
  | "image"
  | "pdf"
  | "video"
  | "audio"
  | "docx"
  | "xlsx"
  | "pptx"
  | "md"
  | "text"
  | "unsupported";

/// 预览请求:urls 各键可缺(容器按格式能力装配);meta 供面板文案。
export interface PreviewRequest {
  kind: PreviewKind;
  urls: {
    /// 原文件 URL。
    raw?: string;
    /// 缩略图 URL(图片渐进首显/列表行共用)。
    thumb?: string;
    /// 转码 JPEG URL(HEIC 等不可直解图;图片查看器优先于 raw)。
    jpeg?: string;
    /// HLS 播放列表 URL(视频;本包暂不消费,契约先钉)。
    hls?: string;
  };
  meta: {
    name: string;
    size: number;
  };
  /// 文案容器缝(T16-M1):容器覆写任一文案键;缺省=包内中文文案。
  strings?: Partial<PreviewStrings>;
}

/// 预览事件(保持最小):
/// - `error`:装配即不可预览(终态,无重试面);
/// - `retry_needed`:渐进加载失败,查看器已停前态并出重试钮(容器可感知/记录);
/// - `meta`:主内容就绪,携载荷。page/pages=页码指示(pdf 渐进装配面;T17 起可选项,
///   image 等旧消费者不携,缺省 undefined 零触碰)。
export type PreviewEvent =
  | { type: "error"; message: string }
  | { type: "retry_needed"; message: string }
  | { type: "meta"; bytes: number; page?: number; pages?: number };

/// 挂载句柄:destroy 摘净装配 DOM、全部监听与在用字节桥。
export interface PreviewHandle {
  destroy(): void;
}
