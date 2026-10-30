import { afterEach, describe, expect, it } from "vitest";

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
  it("THEMES 即循环序 fluent→workbench→dark,默认主题 dark", () => {
    expect(THEMES).toEqual(["fluent", "workbench", "dark"]);
    expect(DEFAULT_THEME).toBe("dark");
  });
});

describe("nextTheme", () => {
  it("三向循环 fluent→workbench→dark,末尾回绕到 fluent", () => {
    expect(nextTheme("fluent")).toBe("workbench");
    expect(nextTheme("workbench")).toBe("dark");
    expect(nextTheme("dark")).toBe("fluent");
  });
});

describe("loadTheme", () => {
  it("无记录 → 默认 dark", () => {
    expect(loadTheme()).toBe("dark");
  });

  it("非法值(「neon」)与空串 → 默认 dark", () => {
    localStorage.setItem(THEME_STORAGE_KEY, "neon");
    expect(loadTheme()).toBe("dark");
    localStorage.setItem(THEME_STORAGE_KEY, "");
    expect(loadTheme()).toBe("dark");
  });

  it("存储不可用(隐私模式,访问即抛)→ 默认 dark 且 saveTheme 不外抛", () => {
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
    expect(() => saveTheme("fluent")).not.toThrow();
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

/// 测试尾清:不留记录,防用例间串场(默认态用例依赖无记录)。
afterEach(() => {
  localStorage.clear();
});
