/// pptx 解析 worker 的内联打包口:@file-viewer/pptx 的解析 worker 资产经 `?raw` 原文内嵌
/// 进单文件 bundle(T16 钉:移动 WebView 拿同一 dist/previewer.js,零旁挂资产文件;真面
/// worker 件是预打包自包含脚本,零 import/export/import.meta,以 blob URL 惰性创建 classic
/// worker,不挂 pptx 不付代价)。独立成件的理由:office.ts 只经本模块取 worker 工厂,
/// 测试侧 vi.mock 本模块即整面摘除 worker 依赖,`?raw` 查询串永不进测试解析面。

import PptxWorkerSource from "@file-viewer/pptx/worker/pptx.worker.js?raw";

export function createPptxParserWorker(): Worker {
  const url = URL.createObjectURL(new Blob([PptxWorkerSource], { type: "text/javascript" }));
  return new Worker(url);
}
