import { describe, expect, it } from "vitest";

import { focusablesIn, initialFocusIndex, nextFocusIndex } from "./focustrap";

/// 便捷构造:最小可聚焦元素 stub(纯数组语义只需要 disabled 字段;
/// querySelectorAll 的 DOM 行为不在 node 环境覆盖,留手动冒烟)。
function focusable(disabled = false): HTMLElement {
  return { disabled } as unknown as HTMLElement;
}

/// 便捷构造:querySelectorAll 原样返回给定元素的 root stub,并捕获
/// 实际下发的选择器串(钉住选择器契约面)。
function rootOf(elements: HTMLElement[]): {
  root: HTMLElement;
  selector: () => string;
} {
  let seen = "";
  const root = {
    querySelectorAll: (sel: string) => {
      seen = sel;
      return elements;
    },
  } as unknown as HTMLElement;
  return { root, selector: () => seen };
}

describe("nextFocusIndex(Tab 环绕索引)", () => {
  it("前进:中间 +1;末尾环绕回首", () => {
    expect(nextFocusIndex(3, 0, false)).toBe(1);
    expect(nextFocusIndex(3, 1, false)).toBe(2);
    expect(nextFocusIndex(3, 2, false)).toBe(0);
  });

  it("Shift 反向:中间 -1;首部环绕到末", () => {
    expect(nextFocusIndex(3, 2, true)).toBe(1);
    expect(nextFocusIndex(3, 1, true)).toBe(0);
    expect(nextFocusIndex(3, 0, true)).toBe(2);
  });

  it("单项容器:前进/后退均停在自身(合法自环绕)", () => {
    expect(nextFocusIndex(1, 0, false)).toBe(0);
    expect(nextFocusIndex(1, 0, true)).toBe(0);
  });

  it("count ≤ 0 → -1(无可聚焦项,调用方吞键防逃逸)", () => {
    expect(nextFocusIndex(0, 0, false)).toBe(-1);
    expect(nextFocusIndex(0, 0, true)).toBe(-1);
    expect(nextFocusIndex(-1, 0, false)).toBe(-1);
  });

  it("current 越界(-1,焦点在非可聚焦子元素上):前进视为从首、Shift 视为从末", () => {
    expect(nextFocusIndex(3, -1, false)).toBe(0);
    expect(nextFocusIndex(3, -1, true)).toBe(2);
  });
});

describe("initialFocusIndex(初始聚焦位)", () => {
  it("有可聚焦项 → 0(首个,惯例取消键);没有 → -1", () => {
    expect(initialFocusIndex(3)).toBe(0);
    expect(initialFocusIndex(1)).toBe(0);
    expect(initialFocusIndex(0)).toBe(-1);
  });
});

describe("focusablesIn(可聚焦收集:stub 面只测纯数组语义)", () => {
  it("保查询序(document order);过滤禁用项(表单族 disabled)", () => {
    const a = focusable(false);
    const b = focusable(true);
    const c = focusable(false);
    const { root } = rootOf([a, b, c]);
    expect(focusablesIn(root)).toEqual([a, c]);
  });

  it("全禁用 → 空表;选择器面钉死契约(button/[href]/input/select/textarea/[tabindex≠-1])", () => {
    const { root, selector } = rootOf([focusable(true)]);
    expect(focusablesIn(root)).toEqual([]);
    expect(selector()).toBe(
      'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
    );
  });
});
