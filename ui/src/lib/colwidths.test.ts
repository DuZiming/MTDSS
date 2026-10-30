import { afterEach, describe, expect, it } from "vitest";

import {
  COL_MAX_WIDTH,
  COL_MIN_WIDTHS,
  COLWIDTH_STORAGE_KEY,
  DEFAULT_COL_WIDTHS,
  clampColWidth,
  loadColWidths,
  saveColWidths,
  sumColWidths,
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
  it("存储键即 mtdss.colw;默认基线 mtime 170 / type 130 / size 90(名称列无默认宽,未拖保持弹性)", () => {
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
    expect(clampColWidth("name", 300.6)).toBe(301);
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

describe("sumColWidths(r2d 表格 min-width 口径:四列当前宽之和)", () => {
  it("全未钉:名称按 clamp 下限 140 计,其余按默认基线 → 530", () => {
    expect(sumColWidths({})).toBe(140 + 170 + 130 + 90);
  });

  it("部分钉住:已存值计入,未钉列按各自回落值", () => {
    expect(sumColWidths({ mtime: 230 })).toBe(140 + 230 + 130 + 90);
    expect(sumColWidths({ name: 300, size: 80 })).toBe(300 + 170 + 130 + 80);
  });

  it("脏值逐键 clamp 后求和(不放大)", () => {
    expect(sumColWidths({ name: 9999, size: 1 })).toBe(999 + 170 + 130 + 70);
  });

  it("非有限数值按回落值处理", () => {
    expect(sumColWidths({ name: Number.NaN })).toBe(140 + 170 + 130 + 90);
  });
});

describe("loadColWidths", () => {
  it("无记录 → 空对象(全列走默认基线/名称弹性)", () => {
    expect(loadColWidths()).toEqual({});
  });

  it("合法记录逐键读出(四键均可,部分自定义也成立)", () => {
    localStorage.setItem(COLWIDTH_STORAGE_KEY, JSON.stringify({ name: 260 }));
    expect(loadColWidths()).toEqual({ name: 260 });
    localStorage.setItem(
      COLWIDTH_STORAGE_KEY,
      JSON.stringify({ name: 260, mtime: 230, type: 100, size: 80 }),
    );
    expect(loadColWidths()).toEqual({ name: 260, mtime: 230, type: 100, size: 80 });
  });

  it("脏值逐键 clamp(超上限/低于下限都归界,合法键不受牵连)", () => {
    localStorage.setItem(
      COLWIDTH_STORAGE_KEY,
      JSON.stringify({ name: 20, mtime: 9999, type: 1, size: 90 }),
    );
    expect(loadColWidths()).toEqual({ name: 140, mtime: 999, type: 90, size: 90 });
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

  it("未知键与非数值丢弃", () => {
    localStorage.setItem(
      COLWIDTH_STORAGE_KEY,
      JSON.stringify({ mtime: 200, foo: 1, size: "80" }),
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
  it("save 后 load 返回同值(四键全量)", () => {
    const w: ColWidths = { name: 300, mtime: 300, type: 120, size: 75 };
    saveColWidths(w);
    expect(loadColWidths()).toEqual(w);
  });

  it("有值才存:未拖过的键(缺省)不落地", () => {
    saveColWidths({ mtime: 300 });
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
