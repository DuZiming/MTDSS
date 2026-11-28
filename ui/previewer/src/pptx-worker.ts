/// pptx 解析 worker 的内联打包口:@file-viewer/pptx 的解析 worker 资产经 `?raw` 原文内嵌
/// 进单文件 bundle(T16 钉:移动 WebView 拿同一 dist/previewer.js,零旁挂资产文件;真面
/// worker 件是预打包自包含脚本,零 import/export/import.meta,以 blob URL 惰性创建 classic
/// worker,不挂 pptx 不付代价)。独立成件的理由:office.ts 只经本模块取 worker 工厂,
/// 测试侧 vi.mock 本模块即整面摘除 worker 依赖,`?raw` 查询串永不进测试解析面。
///
/// CSP 契约(容器必须满足,T20/T22-25 收紧 CSP 时须带上;T26 冒烟单验 pptx 面):
/// 本 worker 以 blob URL 创建(new Worker(URL.createObjectURL(blob)))——容器 CSP 须放行
/// `worker-src blob:`(script-src 兜底亦可)。缺放行 → worker 创建失败 → PptxViewer.open
/// 以 PPTX_WORKER_FAILED 拒 → office 查看器降级 unsupported 形且 error 事件携
/// officeWorkerFailed 专属词+console 记账(本仓 pdf.js 刻意走主线程假工、零 worker,
/// 故 worker-src 是 pptx 独有的活约束;不满足时 pptx 全端静默不可用,唯此词可辨)。

import PptxWorkerSource from "@file-viewer/pptx/worker/pptx.worker.js?raw";

export function createPptxParserWorker(): Worker {
  const url = URL.createObjectURL(new Blob([PptxWorkerSource], { type: "text/javascript" }));
  return new Worker(url);
}
