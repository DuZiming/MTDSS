/// 平铺尺寸档(V4-1 T19)单测:三档序与 Ctrl+滚轮翻档钳制。
import { describe, expect, it } from "vitest";

import { TILE_SIZES, TILE_THUMB_W, stepTileSize, type TileSize } from "./tilesize";

describe("tilesize 三档", () => {
  it("档序恰三档 s<m<l(三档 CSS 变量锚)", () => {
    expect(TILE_SIZES).toEqual(["s", "m", "l"]);
  });

  it("滚轮上(deltaY<0)放大一档,滚轮下缩小一档,端点钳制", () => {
    expect(stepTileSize("m", -100)).toBe("l");
    expect(stepTileSize("m", 100)).toBe("s");
    expect(stepTileSize("l", -100)).toBe("l"); // 顶档封顶
    expect(stepTileSize("s", 100)).toBe("s"); // 底档兜底
  });

  it("deltaY=0(非滚轮位移)不动档", () => {
    expect(stepTileSize("m", 0)).toBe<TileSize>("m");
  });

  it("每档一枚缩略图请求宽(回环口 /thumb?w=,随档升清)", () => {
    expect(TILE_THUMB_W.s).toBeLessThan(TILE_THUMB_W.m);
    expect(TILE_THUMB_W.m).toBeLessThan(TILE_THUMB_W.l);
  });
});
