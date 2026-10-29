import { afterEach, describe, expect, it } from "vitest";

import {
  COL_MAX_WIDTH,
  COL_MIN_WIDTHS,
  COLWIDTH_STORAGE_KEY,
  DEFAULT_COL_WIDTHS,
  clampColWidth,
  loadColWidths,
  saveColWidths,
  type ColWidths,
} from "./colwidths";

/// 内存版 localStorage 桩:vitest 默认 node 环境(无 jsdom)没有
/// localStorage,挂一个同构实现让持久化测试确定性成立(不为此引 jsdom)。
/// colwidth.ts 只用 getItem/setItem,桩按 Storage 接口补全五个方法 + length
/// (与 theme.test.ts 同款)。
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
  it("存储键即 mtdss.colw;默认基线 mtime 170 / type 130 / size 90(名称列恒弹性不预置)", () => {
    expect(COLWIDTH_STORAGE_KEY).toBe("mtdss.colw");
    expect(DEFAULT_COL_WIDTHS).toEqual({ mtime: 170, type: 130, size: 90 });
  });

  it("clamp 下限 名称 140 / 日期 110 / 类型 90 / 大小 70,统一上限 999", () => {
    expect(COL_MIN_WIDTHS).toEqual({ name: 140, mtime: 110, type: 90, size: 70 });
    expect(COL_MAX_WIDTH).toBe(999);
  });
});

describe("clampColWidth", () => {
  it("区间内原样返回并取整(拖动产生的小数 px 归整)", () => {
    expect(clampColWidth("mtime", 200)).toBe(200);
    expect(clampColWidth("type", 130.6)).toBe(131);
    expect(clampColWidth("size", 70.4)).toBe(70);
  });

  it("恰在上下边界取边界值", () => {
    expect(clampColWidth("name", 140)).toBe(140);
    expect(clampColWidth("mtime", 110)).toBe(110);
    expect(clampColWidth("type", 90)).toBe(90);
    expect(clampColWidth("size", 999)).toBe(999);
  });

  it("低于下限取下限,高于 999 取 999", () => {
    expect(clampColWidth("name", 0)).toBe(140);
    expect(clampColWidth("type", 10)).toBe(90);
    expect(clampColWidth("size", -40)).toBe(70);
    expect(clampColWidth("mtime", 5000)).toBe(999);
  });

  it("非有限数值(NaN/Infinity)→ 该列下限,不外抛不放大", () => {
    expect(clampColWidth("mtime", Number.NaN)).toBe(110);
    expect(clampColWidth("type", Number.POSITIVE_INFINITY)).toBe(90);
    expect(clampColWidth("size", Number.NEGATIVE_INFINITY)).toBe(70);
  });
});

describe("loadColWidths", () => {
  it("无记录 → 空对象(全列走默认基线)", () => {
    expect(loadColWidths()).toEqual({});
  });

  it("合法记录逐键读出(部分自定义也成立)", () => {
    localStorage.setItem(COLWIDTH_STORAGE_KEY, JSON.stringify({ mtime: 260 }));
    expect(loadColWidths()).toEqual({ mtime: 260 });
    localStorage.setItem(
      COLWIDTH_STORAGE_KEY,
      JSON.stringify({ mtime: 260, type: 100, size: 80 }),
    );
    expect(loadColWidths()).toEqual({ mtime: 260, type: 100, size: 80 });
  });

  it("脏值逐键 clamp(超上限/低于下限都归界,合法键不受牵连)", () => {
    localStorage.setItem(
      COLWIDTH_STORAGE_KEY,
      JSON.stringify({ mtime: 9999, type: 1, size: 90 }),
    );
    expect(loadColWidths()).toEqual({ mtime: 999, type: 90, size: 90 });
  });

  it("非法 JSON → 空对象", () => {
    localStorage.setItem(COLWIDTH_STORAGE_KEY, "not-json{");
    expect(loadColWidths()).toEqual({});
  });

  it("非对象 JSON(数字/数组/null)→ 空对象", () => {
    localStorage.setItem(COLWIDTH_STORAGE_KEY, "42");
    expect(loadColWidths()).toEqual({});
    localStorage.setItem(COLWIDTH_STORAGE_KEY, "[200]");
    expect(loadColWidths()).toEqual({});
    localStorage.setItem(COLWIDTH_STORAGE_KEY, "null");
    expect(loadColWidths()).toEqual({});
  });

  it("未知键与非数值丢弃;name 键丢弃(名称列恒弹性不存宽)", () => {
    localStorage.setItem(
      COLWIDTH_STORAGE_KEY,
      JSON.stringify({ mtime: 200, name: 300, foo: 1, size: "80" }),
    );
    expect(loadColWidths()).toEqual({ mtime: 200 });
  });

  it("存储不可用(访问即抛)→ 空对象且不外抛", () => {
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
    expect(loadColWidths()).toEqual({});
    Object.defineProperty(globalThis, "localStorage", {
      value: saved,
      configurable: true,
      writable: true,
    });
  });
});

describe("saveColWidths → loadColWidths 回环", () => {
  it("save 后 load 返回同值(三键全量)", () => {
    const w: ColWidths = { mtime: 300, type: 120, size: 75 };
    saveColWidths(w);
    expect(loadColWidths()).toEqual(w);
  });

  it("save 只落地 mtime/type/size(name 即使传入也不落)", () => {
    saveColWidths({ mtime: 300, name: 500 });
    expect(loadColWidths()).toEqual({ mtime: 300 });
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
    expect(() => saveColWidths({ mtime: 300 })).not.toThrow();
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
