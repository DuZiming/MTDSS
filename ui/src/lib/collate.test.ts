import { describe, expect, it } from "vitest";

import { codePointCmp, zhCmp } from "./collate";

/// 符号归一:Math.sign 对 -0 返回 -0,与 +0 的 Object.is 不等;归一到 0/±1。
const sgn = (n: number): number => Math.sign(n) || 0;

describe("codePointCmp", () => {
  it("自反:相同串比较恒为 0(含空串与 CJK)", () => {
    for (const s of ["", "a", "abc", "台式机", "é"]) {
      expect(codePointCmp(s, s)).toBe(0);
    }
  });

  it("反对称:sign(cmp(a,b)) === -sign(cmp(b,a))", () => {
    const pairs: Array<[string, string]> = [
      ["a", "b"],
      ["b", "a"],
      ["A", "a"],
      ["10", "9"],
      ["", "a"],
      ["台式机", "笔记本"],
      ["apple", "Apple"],
    ];
    for (const [a, b] of pairs) {
      expect(sgn(codePointCmp(a, b)) + sgn(codePointCmp(b, a))).toBe(0);
    }
  });

  it("UTF-16 码元序:大小写敏感、数字按字典序(跨环境确定)", () => {
    expect(codePointCmp("a", "b")).toBeLessThan(0);
    expect(codePointCmp("B", "a")).toBeLessThan(0); // 大写码元 < 小写码元
    expect(codePointCmp("10", "9")).toBeLessThan(0); // 字典序而非数值序
    expect(codePointCmp("", "a")).toBeLessThan(0);
  });
});

describe("zhCmp", () => {
  it("构造不抛异常,返回函数(能力缺失时回退码元序也满足契约)", () => {
    expect(typeof zhCmp()).toBe("function");
  });

  it("任意正确 collator 均满足:自反与反对称(只断言环境无关性质)", () => {
    const cmp = zhCmp();
    const samples = ["a", "z", "A", "9", "10", "台式机", "笔记本", "banana", "apple", ""];
    for (const s of samples) {
      expect(cmp(s, s)).toBe(0);
    }
    for (const a of samples) {
      for (const b of samples) {
        expect(sgn(cmp(a, b)) + sgn(cmp(b, a))).toBe(0);
      }
    }
  });

  it("ASCII 名称按字典序(任何正确 collator 皆然)", () => {
    const cmp = zhCmp();
    const out = [...["banana", "apple", "cherry", "apple"]].sort(cmp);
    expect(out).toEqual(["apple", "apple", "banana", "cherry"]);
  });
});
