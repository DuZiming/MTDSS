import { afterEach, describe, expect, it } from "vitest";

import zh from "../i18n/zh.json";
import {
  DEFAULT_THEME,
  THEMES,
  THEME_STORAGE_KEY,
  loadTheme,
  nextTheme,
  saveTheme,
} from "./theme";

/// 内存版 localStorage 桩:vitest 默认 node 环境(无 jsdom)没有
/// localStorage,挂一个同构实现让持久化测试确定性成立(不为此引 jsdom)。
/// theme.ts 只用 getItem/setItem,桩按 Storage 接口补全五个方法 + length。
const store = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", {
  value: {
    get length(): number {
      return store.size;
    },
    clear(): void {
      store.clear();
    },
    getItem(key: string): string | null {
      return store.has(key) ? (store.get(key) as string) : null;
    },
    key(index: number): string | null {
      return [...store.keys()][index] ?? null;
    },
    removeItem(key: string): void {
      store.delete(key);
    },
    setItem(key: string, value: string): void {
      store.set(key, String(value));
    },
  },
  configurable: true,
  writable: true,
});

describe("常量与循环序", () => {
  it("THEMES 即循环序 light→dark 两态,默认主题 light(spec §5.1-6:两套默认白)", () => {
    expect(THEMES).toEqual(["light", "dark"]);
    expect(DEFAULT_THEME).toBe("light");
  });
});

describe("nextTheme", () => {
  it("两态循环 light→dark,末尾回绕到 light", () => {
    expect(nextTheme("light")).toBe("dark");
    expect(nextTheme("dark")).toBe("light");
  });
});

describe("loadTheme 迁移(V3-6 三主题→两主题收敛)", () => {
  it("存量 dark 原样保留", () => {
    localStorage.setItem(THEME_STORAGE_KEY, "dark");
    expect(loadTheme()).toBe("dark");
  });

  it("存量 fluent → light(迁移表)", () => {
    localStorage.setItem(THEME_STORAGE_KEY, "fluent");
    expect(loadTheme()).toBe("light");
  });

  it("存量 workbench → light(迁移表;亮档承其全套 token)", () => {
    localStorage.setItem(THEME_STORAGE_KEY, "workbench");
    expect(loadTheme()).toBe("light");
  });

  it("缺失记录 → 默认 light", () => {
    expect(loadTheme()).toBe("light");
  });

  it("空串与非法值(「neon」)→ 默认 light", () => {
    localStorage.setItem(THEME_STORAGE_KEY, "");
    expect(loadTheme()).toBe("light");
    localStorage.setItem(THEME_STORAGE_KEY, "neon");
    expect(loadTheme()).toBe("light");
  });

  it("存储不可用(隐私模式,访问即抛)→ 默认 light 且 saveTheme 不外抛", () => {
    const saved = globalThis.localStorage;
    Object.defineProperty(globalThis, "localStorage", {
      value: {
        get length(): number {
          throw new Error("denied");
        },
        clear(): void {
          throw new Error("denied");
        },
        getItem(): string | null {
          throw new Error("denied");
        },
        key(): string | null {
          throw new Error("denied");
        },
        removeItem(): void {
          throw new Error("denied");
        },
        setItem(): void {
          throw new Error("denied");
        },
      },
      configurable: true,
    });
    expect(loadTheme()).toBe(DEFAULT_THEME);
    expect(() => saveTheme("dark")).not.toThrow();
    Object.defineProperty(globalThis, "localStorage", {
      value: saved,
      configurable: true,
      writable: true,
    });
  });
});

describe("saveTheme → loadTheme 回读", () => {
  it("save 任一合法主题后 load 返回同值", () => {
    for (const t of THEMES) {
      saveTheme(t);
      expect(loadTheme()).toBe(t);
    }
  });
});

describe("i18n 词条同步门", () => {
  it("zh 主题词条键与 THEMES(+cycle)严格一致:旧 id 词条残留或新 id 缺词条即红", () => {
    const labels = Object.keys((zh as { theme: Record<string, string> }).theme);
    expect([...labels].sort()).toEqual([...THEMES, "cycle"].sort());
  });
});

/// 测试尾清:不留记录,防用例间串场(默认态用例依赖无记录)。
afterEach(() => {
  localStorage.clear();
});
