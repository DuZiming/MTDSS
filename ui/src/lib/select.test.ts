import { describe, expect, it } from "vitest";

import { applyClickSelect, clickMode, hasSelectModifier, rubberBandSelect } from "./select";

/// 当前渲染序的便捷路径表(测试用;与组件层 rows.map(r => r.path) 同构)。
const paths = ["a", "b", "c", "d", "e"];

describe("applyClickSelect / 单击", () => {
  it("清空为该行单选,锚点移至该行", () => {
    const out = applyClickSelect(new Set(["b", "c"]), paths, "d", "single", "b");
    expect([...out.selection]).toEqual(["d"]);
    expect(out.anchor).toBe("d");
  });
});

describe("applyClickSelect / Ctrl", () => {
  it("未选 → 加入选择集", () => {
    const out = applyClickSelect(new Set(["a"]), paths, "c", "ctrl", "a");
    expect([...out.selection].sort()).toEqual(["a", "c"]);
    expect(out.anchor).toBe("c");
  });

  it("已选 → 从选择集中移除", () => {
    const out = applyClickSelect(new Set(["a", "c"]), paths, "c", "ctrl", "c");
    expect([...out.selection]).toEqual(["a"]);
    expect(out.anchor).toBe("c");
  });

  it("不改入参集合", () => {
    const cur = new Set(["a"]);
    applyClickSelect(cur, paths, "c", "ctrl", null);
    expect([...cur]).toEqual(["a"]);
  });
});

describe("applyClickSelect / Shift", () => {
  it("正向区间:锚点在前,闭区间替换选择", () => {
    const out = applyClickSelect(new Set(["a"]), paths, "d", "shift", "b");
    expect([...out.selection].sort()).toEqual(["b", "c", "d"]);
  });

  it("反向区间:锚点在后,同样取闭区间", () => {
    const out = applyClickSelect(new Set(), paths, "b", "shift", "d");
    expect([...out.selection].sort()).toEqual(["b", "c", "d"]);
  });

  it("锚点不动:连续 Shift 扩展仍以原锚点为基准", () => {
    const first = applyClickSelect(new Set(), paths, "c", "shift", "a");
    const second = applyClickSelect(first.selection, paths, "e", "shift", first.anchor);
    expect([...second.selection].sort()).toEqual(["a", "b", "c", "d", "e"]);
    expect(second.anchor).toBe("a");
  });

  it("端点重合:区间即锚点行自身", () => {
    const out = applyClickSelect(new Set(["a", "e"]), paths, "c", "shift", "c");
    expect([...out.selection]).toEqual(["c"]);
  });

  it("锚点缺失(null)→ 退化为单击", () => {
    const out = applyClickSelect(new Set(["a"]), paths, "d", "shift", null);
    expect([...out.selection]).toEqual(["d"]);
    expect(out.anchor).toBe("d");
  });

  it("锚点不在当前渲染序中(过滤/重排后失配)→ 退化为单击", () => {
    const out = applyClickSelect(new Set(["a"]), paths, "d", "shift", "zz");
    expect([...out.selection]).toEqual(["d"]);
    expect(out.anchor).toBe("d");
  });
});

describe("clickMode / hasSelectModifier", () => {
  it("Shift 优先于 Ctrl(Ctrl+Shift 并按视为范围扩展)", () => {
    expect(clickMode({ shiftKey: true, ctrlKey: false, metaKey: false })).toBe("shift");
    expect(clickMode({ shiftKey: true, ctrlKey: true, metaKey: false })).toBe("shift");
    expect(clickMode({ shiftKey: false, ctrlKey: true, metaKey: false })).toBe("ctrl");
    expect(clickMode({ shiftKey: false, ctrlKey: false, metaKey: true })).toBe("ctrl");
    expect(clickMode({ shiftKey: false, ctrlKey: false, metaKey: false })).toBe("single");
  });

  it("任一修饰键即视为多选手势,双击不得进入目录", () => {
    expect(hasSelectModifier({ shiftKey: false, ctrlKey: false, metaKey: false })).toBe(false);
    expect(hasSelectModifier({ shiftKey: true, ctrlKey: false, metaKey: false })).toBe(true);
    expect(hasSelectModifier({ shiftKey: false, ctrlKey: true, metaKey: false })).toBe(true);
    expect(hasSelectModifier({ shiftKey: false, ctrlKey: false, metaKey: true })).toBe(true);
  });
});

describe("rubberBandSelect / 框选", () => {
  /// 行矩形表:五行等高纵排(x 0..300,每行高 24),与视口测量同构。
  const rowRects = paths.map((p, i) => ({
    path: p,
    rect: { x: 0, y: i * 24, width: 300, height: 24 },
  }));

  it("rubber_band_intersects_rows:框罩住的行入选,贴边不误选", () => {
    // 框 y ∈ [24,72]:完整覆盖 b、c 两行;与 a(0..24)、d(72..96)仅贴边
    // (零高度重叠)→ 排除。
    const out = rubberBandSelect({ x: 10, y: 24 }, { x: 200, y: 72 }, rowRects, new Set());
    expect([...out].sort()).toEqual(["b", "c"]);
  });

  it("rubber_band_empty_clears:零面积框(未拖动)清空选择", () => {
    const out = rubberBandSelect({ x: 10, y: 24 }, { x: 10, y: 24 }, rowRects, new Set(["a", "b"]));
    expect(out.size).toBe(0);
  });

  it("rubber_band_does_not_disturb_anchor:锚点语义不进框选,结果只由几何+prev 决定", () => {
    // Shift 范围选择(锚点 b → e)产生的 prev;框选启动时 App 清锚点,
    // 纯函数既不读锚点也不生成新锚点(返回纯集合,锚点清理由调用方负责)。
    const shift = applyClickSelect(new Set(), paths, "e", "shift", "b"); // {b,c,d,e}
    // 框只罩住 a:结果 = prev ∪ {a}(非零面积框对 prev 求并)。
    const out = rubberBandSelect({ x: 10, y: 0 }, { x: 200, y: 24 }, rowRects, shift.selection);
    expect([...out].sort()).toEqual(["a", "b", "c", "d", "e"]);
    // 对照:同一几何、不同 prev(单条)——结果只随 prev 变化,与锚点历史无关。
    const out2 = rubberBandSelect({ x: 10, y: 0 }, { x: 200, y: 24 }, rowRects, new Set(["d"]));
    expect([...out2].sort()).toEqual(["a", "d"]);
  });
});
