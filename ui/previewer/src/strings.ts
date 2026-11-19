/// 文案容器缝(T16-M1,T17 落地):包内所有用户可见文案收敛本目录。
/// 容器(桌面 T20 / 移动 T23/T25)经 `PreviewRequest.strings` 覆写任一键,
/// 未覆写键取缺省中文;模板键以 `{page}`/`{total}` 词位插值(纯字符串,可序列化,
/// 移动容器经 JSON 注入)。缺省词 = T16 既有文案原词 + T17 新增面。

/// 文案目录:键即语义位;值=展示词(模板键含词位)。
export interface PreviewStrings {
  /// 通用:重试钮。
  retry: string;
  /// 通用:装配即不可预览的 error 事件词。
  unavailableMessage: string;
  /// 图片:渐进失败覆盖层文案。
  imageLoadFailed: string;
  /// 图片:渐进失败 retry_needed 事件词。
  imageRetryNeeded: string;
  /// 图片:装配即不可预览覆盖层文案。
  imageUnavailable: string;
  /// 占位面板:格式未支持文案。
  unsupportedNote: string;
  /// 占位面板:raw 在时的下载链接词。
  downloadToView: string;
  /// 占位面板:raw 缺时的静态引导词。
  downloadHint: string;
  /// pdf:页码指示模板({page}/{total} 词位)。
  pdfPageIndicator: string;
  /// pdf:续载钮(滚动近底亦可触发,钮为显式入口)。
  pdfLoadMore: string;
  /// pdf:加载失败覆盖层/事件词。
  pdfLoadFailed: string;
  /// 文本(md/text):续读钮。
  textLoadMore: string;
  /// 文本(md/text):加载失败覆盖层/事件词。
  textLoadFailed: string;
  /// office(docx/xlsx/pptx):整取/解析期间的加载面文案。
  officeLoading: string;
  /// office(docx):「查看更多」续渲钮。
  officeLoadMore: string;
  /// office(xlsx):「加载更多」续行钮。
  xlsxLoadMore: string;
  /// office:解析/渲染失败降级 unsupported 形时的 error 事件词(终态,无重试面)。
  officeLoadFailed: string;
}

/// 缺省文案(T16 image/占位原词逐字迁移;pdf/text 为 T17 新增;office 为 T18 新增)。
export const defaultPreviewStrings: PreviewStrings = {
  retry: "重试",
  unavailableMessage: "无可用预览地址",
  imageLoadFailed: "图片加载失败",
  imageRetryNeeded: "原图加载失败",
  imageUnavailable: "图片不可预览",
  unsupportedNote: "该格式暂未支持",
  downloadToView: "下载后查看",
  downloadHint: "可下载后查看",
  pdfPageIndicator: "第 {page} / {total} 页",
  pdfLoadMore: "查看更多",
  pdfLoadFailed: "PDF 加载失败",
  textLoadMore: "加载更多",
  textLoadFailed: "文本加载失败",
  officeLoading: "正在加载…",
  officeLoadMore: "查看更多",
  xlsxLoadMore: "加载更多",
  officeLoadFailed: "文档预览失败",
};

/// 缝的合并点:partial 覆盖缺省,产出完整文案(查看器收全量,免判空)。
export function resolvePreviewStrings(partial?: Partial<PreviewStrings>): PreviewStrings {
  return { ...defaultPreviewStrings, ...partial };
}

/// 页码模板插值:`{page}`/`{total}` 词位全量替换(可重复出现)。
export function formatPdfPageIndicator(template: string, page: number, total: number): string {
  return template.replace(/\{page\}/g, String(page)).replace(/\{total\}/g, String(total));
}
