/// 装配面样式:文档级单例 `<style id="mtdss-preview-styles">` 注入(幂等)。
///
/// CSP 兼容:无内联 eval;`<style>` 走 style-src(桌面 Tauri 面 'unsafe-inline' 在册),
/// 移动端 WebView 打包同文件照用。
///
/// 无全局污染:所有选择器都以 `.mtdss-preview` 前缀 scoped(测试钉面)。
/// 零硬编码业务色:仅中性灰阶令牌,主题适配由容器覆写 `--mtdss-preview-*`
/// CSS 变量(暗色主题=容器换令牌,包内不感知)。

const STYLE_ID = "mtdss-preview-styles";

const CSS = `
.mtdss-preview {
  --_bg: var(--mtdss-preview-bg, #f7f7f6);
  --_fg: var(--mtdss-preview-fg, #3d3d3d);
  --_muted: var(--mtdss-preview-muted, #8a8a88);
  --_border: var(--mtdss-preview-border, #d9d9d6);
  --_control-bg: var(--mtdss-preview-control-bg, #ffffff);
  position: relative;
  display: flex;
  flex-direction: column;
  width: 100%;
  height: 100%;
  overflow: hidden;
  background: var(--_bg);
  color: var(--_fg);
  font-size: 14px;
  line-height: 1.5;
}
.mtdss-preview__stage {
  flex: 1;
  min-height: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  cursor: grab;
}
.mtdss-preview__stage--dragging {
  cursor: grabbing;
}
.mtdss-preview__img {
  max-width: 100%;
  max-height: 100%;
  user-select: none;
  transform-origin: center center;
  will-change: transform;
}
.mtdss-preview__overlay {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  background: var(--_bg);
}
.mtdss-preview__overlay--hidden {
  display: none;
}
.mtdss-preview__note {
  color: var(--_muted);
}
.mtdss-preview__button {
  padding: 6px 20px;
  border: 1px solid var(--_border);
  border-radius: 6px;
  background: var(--_control-bg);
  color: var(--_fg);
  font: inherit;
  cursor: pointer;
}
.mtdss-preview__button:active {
  background: var(--_bg);
}
.mtdss-preview__placeholder {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 24px;
  text-align: center;
}
.mtdss-preview__placeholder-name {
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 600;
}
.mtdss-preview__placeholder-note {
  color: var(--_muted);
}
.mtdss-preview__download {
  margin-top: 8px;
}
.mtdss-preview__download-link {
  color: inherit;
}
.mtdss-preview__bar {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 12px;
  padding: 8px 12px;
  border-top: 1px solid var(--_border);
}
.mtdss-preview__indicator {
  color: var(--_muted);
  font-variant-numeric: tabular-nums;
}
.mtdss-preview__pdf-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 12px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
}
.mtdss-preview__pdf-page {
  width: 100%;
  max-width: 860px;
}
.mtdss-preview__pdf-canvas {
  display: block;
  width: 100%;
  height: auto;
  background: var(--_control-bg);
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.15);
}
.mtdss-preview__text-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 16px;
}
.mtdss-preview__md {
  max-width: 860px;
  margin: 0 auto;
  overflow-wrap: break-word;
}
.mtdss-preview__md h1 {
  font-size: 1.5em;
  margin: 0.6em 0 0.4em;
}
.mtdss-preview__md h2 {
  font-size: 1.3em;
  margin: 0.6em 0 0.4em;
}
.mtdss-preview__md h3,
.mtdss-preview__md h4 {
  font-size: 1.1em;
  margin: 0.6em 0 0.3em;
}
.mtdss-preview__md p {
  margin: 0.4em 0;
}
.mtdss-preview__md ul,
.mtdss-preview__md ol {
  margin: 0.4em 0;
  padding-left: 1.6em;
}
.mtdss-preview__md blockquote {
  margin: 0.5em 0;
  padding: 0.2em 0.8em;
  border-left: 3px solid var(--_border);
  color: var(--_muted);
}
.mtdss-preview__md code {
  font-family: monospace;
  font-size: 0.92em;
  background: var(--_control-bg);
  border: 1px solid var(--_border);
  border-radius: 3px;
  padding: 0.1em 0.35em;
}
.mtdss-preview__md pre {
  background: var(--_control-bg);
  border: 1px solid var(--_border);
  border-radius: 6px;
  padding: 10px 12px;
  overflow-x: auto;
}
.mtdss-preview__md pre code {
  background: transparent;
  border: none;
  padding: 0;
}
.mtdss-preview__md img {
  max-width: 100%;
}
.mtdss-preview__md table {
  border-collapse: collapse;
  margin: 0.5em 0;
}
.mtdss-preview__md th,
.mtdss-preview__md td {
  border: 1px solid var(--_border);
  padding: 4px 10px;
}
.mtdss-preview__md a {
  color: inherit;
}
.mtdss-preview__code {
  max-width: 980px;
  margin: 0 auto;
  background: var(--_control-bg);
  border: 1px solid var(--_border);
  border-radius: 6px;
  padding: 10px 12px;
  overflow-x: auto;
}
.mtdss-preview__code-code {
  display: block;
  white-space: pre;
  font-family: monospace;
  font-size: 13px;
  line-height: 1.5;
}
`;

/// 幂等注入:已存在即跳过(多实例/重复挂载共享一份)。
/// destroy 不摘样式——文档级单例,在用实例共享,摘除会殃及他实例。
export function ensureStyles(): void {
  if (document.getElementById(STYLE_ID) !== null) return;
  const style = document.createElement("style");
  style.id = STYLE_ID;
  style.textContent = CSS;
  document.head.appendChild(style);
}
