/// 主题基建(V3-6 Task 17,spec §5.1-6 + 裁量 #8):两主题 id、循环次序
/// 与 localStorage 持久化。主题是纯观感偏好,所有 localStorage 访问都
/// try/catch 包裹(隐私模式/存储被禁时静默回退,不影响任何功能);token
/// 值本体在 themes.css(亮档承 workbench 全表 verbatim),本模块只管
/// 「当前是哪个主题」。

/// 两主题:light 工具台白 / dark 夜间监控台。
export type ThemeId = "light" | "dark";

/// 循环序(工具栏循环按钮按此推进):light→dark→light,两态即开关。
export const THEMES: ThemeId[] = ["light", "dark"];

/// 默认主题:light(spec §5.1-6:两套默认白;无记录/非法值落点)。
export const DEFAULT_THEME: ThemeId = "light";

/// 持久化键(设计 §4:localStorage["mtdss.theme"],键名跨收敛不变)。
export const THEME_STORAGE_KEY = "mtdss.theme";

/// 主题循环:THEMES 序中取下一个,末尾回绕到队首。
export function nextTheme(t: ThemeId): ThemeId {
  return THEMES[(THEMES.indexOf(t) + 1) % THEMES.length];
}

/// 读取持久化主题并迁移(P4b 三主题存量 → V3-6 两主题):dark 原样
/// 保留;fluent/workbench 并入 light;缺失/非法值(含空串)→ DEFAULT;
/// 存储不可用(隐私模式等访问即抛)同样按 DEFAULT,绝不外抛。迁移结果
/// 由 App 挂载 effect 的 saveTheme 回写落库。启动路径(main.tsx 挂载前
/// 同步调用)与 App 初始 state 同源,保证首帧即正确主题、无闪烁。
export function loadTheme(): ThemeId {
  try {
    const raw = localStorage.getItem(THEME_STORAGE_KEY);
    if (raw != null && (THEMES as string[]).includes(raw)) {
      return raw as ThemeId;
    }
    // P4b 存量合法值仅剩这两枚:亮档承 workbench token,统一并入 light
    if (raw === "fluent" || raw === "workbench") {
      return "light";
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
