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
    /// HLS 播放列表 URL(视频;video 查看器消费,T21 起真接线)。
    hls?: string;
  };
  meta: {
    name: string;
    size: number;
    /// 稳定文件键(r45 封面持久缓存:`path|mtime|size`)。桌面/iOS
    /// boot 携;缺席=封面缓存跳过(行为=现状),向后兼容。
    key?: string;
  };
  /// 文案容器缝(T16-M1):容器覆写任一文案键;缺省=包内中文文案。
  strings?: Partial<PreviewStrings>;
  /// r38 直读形态旗(iOS 容器注入):true = 原档经 fetch→blob 再喂
  /// `<video>`(WebKit 媒体进程经自定义 scheme 加载远端流会挂起——
  /// 本地已通、远端看门狗超时的实测定制;fetch 走网页进程已证畅通)。
  /// 缺省 false = video.src = raw URL 直连(桌面形态)。
  directBlob?: boolean;
  /// r49 #2 autoPlay 旗(iOS 视频相册化:原生海报点按进播放):true =
  /// 元数据就绪即试 `video.play()`——WebKit 用户手势策略拒则静默落
  /// 控制条单击播(降级面,不视为失败)。缺省 false = 现状(手动播)。
  autoPlay?: boolean;
}

/// 预览事件(保持最小):
/// - `error`:装配即不可预览(终态,无重试面);
/// - `retry_needed`:渐进加载失败,查看器已停前态并出重试钮(容器可感知/记录);
/// - `meta`:主内容就绪,携载荷。page/pages=页码指示(pdf 渐进装配面;T17 起可选项,
///   image 等旧消费者不携,缺省 undefined 零触碰)。
export type PreviewEvent =
  | { type: "error"; message: string; detail?: string }
  | { type: "retry_needed"; message: string }
  | { type: "meta"; bytes: number; page?: number; pages?: number }
  /// 图片查看器导航(field 2026-11-23):上一张/下一张(容器提供 nav 时显示)。
  | { type: "nav"; dir: "prev" | "next" };

/// 挂载句柄:destroy 摘净装配 DOM、全部监听与在用字节桥。
/// mountPreview 第 4 参 nav(可选):提供则图片查看器显示上一张/下一张
/// (nav 事件由容器消费);缺省隐藏导航钮(单文件弹窗等无序列场景)。
export interface PreviewHandle {
  destroy(): void;
}
