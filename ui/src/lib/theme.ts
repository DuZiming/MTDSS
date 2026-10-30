/// 主题基建(P4b Task 1,设计 §4):三主题 id、循环次序与 localStorage
/// 持久化。主题是纯观感偏好,所有 localStorage 访问都 try/catch 包裹
/// (隐私模式/存储被禁时静默回退,不影响任何功能);token 值本体在
/// themes.css(spec §4 全表逐值),本模块只管「当前是哪个主题」。

/// 三主题:A fluent 原生 / B workbench 工具台 / C dark 夜间监控台。
export type ThemeId = "fluent" | "workbench" | "dark";

/// 循环序(工具栏循环按钮按此推进):fluent→workbench→dark→fluent。
export const THEMES: ThemeId[] = ["fluent", "workbench", "dark"];

/// 默认主题:dark 夜间监控台(无记录/记录非法时的落点,设计 §4)。
export const DEFAULT_THEME: ThemeId = "dark";

/// 持久化键(设计 §4:localStorage["mtdss.theme"])。
export const THEME_STORAGE_KEY = "mtdss.theme";

/// 主题循环:THEMES 序中取下一个,末尾回绕到队首。
export function nextTheme(t: ThemeId): ThemeId {
  return THEMES[(THEMES.indexOf(t) + 1) % THEMES.length];
}

/// 读取持久化主题:缺失/非法值(含空串)→ DEFAULT;存储不可用(隐私
/// 模式等访问即抛)同样按 DEFAULT,绝不外抛。启动路径(main.tsx 挂载前
/// 同步调用)与 App 初始 state 同源,保证首帧即正确主题、无闪烁。
export function loadTheme(): ThemeId {
  try {
    const raw = localStorage.getItem(THEME_STORAGE_KEY);
    if (raw != null && (THEMES as string[]).includes(raw)) {
      return raw as ThemeId;
    }
  } catch {
    // 存储不可用 → 缺省主题(与 settings 损坏兜底同口径)
  }
  return DEFAULT_THEME;
}

/// 持久化主题:写失败(隐私模式/配额)静默——当前会话内 data-theme 已
/// 生效,只是下次冷启动不记忆,不值得为此打扰用户。
export function saveTheme(t: ThemeId): void {
  try {
    localStorage.setItem(THEME_STORAGE_KEY, t);
  } catch {
    // 写失败静默:会话内仍生效,仅不跨会话记忆
  }
}
