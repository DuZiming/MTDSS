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
