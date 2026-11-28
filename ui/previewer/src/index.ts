/// r70e 旧引擎运行时内建垫(必须最先求值:单文件 bundle 顶层序即执行序,
/// 后续全部供应商码消费这些内建)——见 compat.ts 注记。
import "./compat";

export { mountCover } from "./cover";
export type { CoverHandle } from "./cover";
export { COVER_OFFICE_MAX_BYTES } from "./cover";
/// 样式(2026-11-23 CSP 根因修):静态 css 资产——桌面 Tauri 面被服务的
/// CSP 会拒运行时注入的 <style>(sheet 恒 null,弹窗全 UI 未样式化根因);
/// 构建期 css 经 vite 入各入口 css chunk,以 <link> 同源加载(style-src
/// 'self' 放行)。消费方(previewer-main 等)按需 `@mtdss/previewer/styles.css`。
import "./styles.css";

export { mountPreview } from "./router";
export {
  defaultPreviewStrings,
  formatPdfPageIndicator,
  resolvePreviewStrings,
} from "./strings";
export type { PreviewStrings } from "./strings";
export type {
  PreviewEvent,
  PreviewHandle,
  PreviewKind,
  PreviewRequest,
} from "./types";
