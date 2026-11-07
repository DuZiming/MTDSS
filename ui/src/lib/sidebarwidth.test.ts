import { afterEach, describe, expect, it } from "vitest";

import {
  DEFAULT_SIDEBAR_WIDTH,
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_MIN_WIDTH,
  SIDEBAR_WIDTH_STORAGE_KEY,
  clampSidebarWidth,
  loadSidebarWidth,
  saveSidebarWidth,
} from "./sidebarwidth";

/// 内存版 localStorage 桩:vitest 默认 node 环境(无 jsdom)没有
/// localStorage,挂一个同构实现让持久化测试确定性成立(不为此引 jsdom)。
/// colwidths.test.ts / theme.test.ts 同款。
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

describe("常量基线", () => {
  it("存储键即 sidebarWidth(§17.4 键面钦定);区间 160-420,兜底默认 196(A/C 主题 --side-w 存档值)", () => {
    expect(SIDEBAR_WIDTH_STORAGE_KEY).toBe("sidebarWidth");
    expect(SIDEBAR_MIN_WIDTH).toBe(160);
    expect(SIDEBAR_MAX_WIDTH).toBe(420);
    expect(DEFAULT_SIDEBAR_WIDTH).toBe(196);
  });
});

describe("clampSidebarWidth", () => {
  it("区间内原样返回并取整(拖动产生的小数 px 归整)", () => {
    expect(clampSidebarWidth(280)).toBe(280);
    expect(clampSidebarWidth(240.6)).toBe(241);
    expect(clampSidebarWidth(239.4)).toBe(239);
  });

  it("恰在上下边界取边界值", () => {
    expect(clampSidebarWidth(160)).toBe(160);
    expect(clampSidebarWidth(420)).toBe(420);
  });

  it("低于 160 取 160,高于 420 取 420", () => {
    expect(clampSidebarWidth(0)).toBe(160);
    expect(clampSidebarWidth(-40)).toBe(160);
    expect(clampSidebarWidth(9999)).toBe(420);
  });

  it("非有限数值(NaN/±Infinity)→ 默认 196,不外抛不放大", () => {
    expect(clampSidebarWidth(Number.NaN)).toBe(196);
    expect(clampSidebarWidth(Number.POSITIVE_INFINITY)).toBe(196);
    expect(clampSidebarWidth(Number.NEGATIVE_INFINITY)).toBe(196);
  });
});

describe("loadSidebarWidth", () => {
  it("无记录 → null(语义=未自定义,壳宽回落逐主题默认 var(--side-w))", () => {
    expect(loadSidebarWidth()).toBeNull();
  });

  it("合法数值记录读出(存储为数值 JSON)", () => {
    localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, "280");
    expect(loadSidebarWidth()).toBe(280);
    localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, "161");
    expect(loadSidebarWidth()).toBe(161);
  });

  it("脏值 clamp(越界归界,不放大)", () => {
    localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, "20");
    expect(loadSidebarWidth()).toBe(160);
    localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, "9999");
    expect(loadSidebarWidth()).toBe(420);
    localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, "240.7");
    expect(loadSidebarWidth()).toBe(241);
  });

  it("非法 JSON → null", () => {
    localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, "not-json{");
    expect(loadSidebarWidth()).toBeNull();
  });

  it("非数值 JSON(字符串/布尔/数组/对象/null)→ null", () => {
    localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, '"280"');
    expect(loadSidebarWidth()).toBeNull();
    localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, "true");
    expect(loadSidebarWidth()).toBeNull();
    localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, "[280]");
    expect(loadSidebarWidth()).toBeNull();
    localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, '{"w":280}');
    expect(loadSidebarWidth()).toBeNull();
    localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, "null");
    expect(loadSidebarWidth()).toBeNull();
  });

  it("存储不可用(访问即抛)→ null 且不外抛", () => {
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
    expect(loadSidebarWidth()).toBeNull();
    Object.defineProperty(globalThis, "localStorage", {
      value: saved,
      configurable: true,
      writable: true,
    });
  });
});

describe("saveSidebarWidth → loadSidebarWidth 回环", () => {
  it("save 后 load 返回同值", () => {
    saveSidebarWidth(320);
    expect(loadSidebarWidth()).toBe(320);
  });

  it("save 先 clamp:越界/小数落存储即干净值", () => {
    saveSidebarWidth(9999);
    expect(localStorage.getItem(SIDEBAR_WIDTH_STORAGE_KEY)).toBe("420");
    saveSidebarWidth(199.6);
    expect(localStorage.getItem(SIDEBAR_WIDTH_STORAGE_KEY)).toBe("200");
  });

  it("存储不可用 → save 不外抛(静默回退)", () => {
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
    expect(() => saveSidebarWidth(320)).not.toThrow();
    Object.defineProperty(globalThis, "localStorage", {
      value: saved,
      configurable: true,
      writable: true,
    });
  });
});

/// 测试尾清:不留记录,防用例间串场(默认态用例依赖无记录)。
afterEach(() => {
  localStorage.clear();
});
