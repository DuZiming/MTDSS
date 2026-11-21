import { describe, expect, it } from "vitest";

import { playlistHasSegments, playlistTargetDurationSecs } from "./playlist";

/// playlist 纯解析(T15-M7 转达钉面):首刷纯头无 EXTINF → 按 TARGETDURATION
/// 轮询重载;解析面单点,消费面 = video.ts 轮询环。

describe("playlistTargetDurationSecs(TARGETDURATION 解析)", () => {
  it("在册值解析(daemon 恒 4000ms → 4)", () => {
    const text = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:4\n";
    expect(playlistTargetDurationSecs(text)).toBe(4);
  });

  it("缺省回退 4(无 TARGETDURATION 行)", () => {
    expect(playlistTargetDurationSecs("#EXTM3U\n")).toBe(4);
  });

  it("畸形值:在册但 <1 钳最小 1(零除防护);非数视同缺行回退 4", () => {
    expect(playlistTargetDurationSecs("#EXT-X-TARGETDURATION:0\n")).toBe(1);
    expect(playlistTargetDurationSecs("#EXT-X-TARGETDURATION:-2\n")).toBe(1);
    expect(playlistTargetDurationSecs("#EXT-X-TARGETDURATION:abc\n")).toBe(4);
  });
});

describe("playlistHasSegments(零段纯头判别,T15-M7)", () => {
  it("纯头形(仅 attach/init,pulled≤1)无 EXTINF = false", () => {
    const text =
      '#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-PLAYLIST-TYPE:EVENT\n#EXT-X-TARGETDURATION:4\n' +
      '#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-MAP:URI="seg/0.m4s"\n';
    expect(playlistHasSegments(text)).toBe(false);
  });

  it("首段出现(EXTINF 在场)= true(轮询环退出条件)", () => {
    const text =
      "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MAP:URI=\"seg/0.m4s\"\n" +
      "#EXTINF:4.000,\nseg/1.m4s\n";
    expect(playlistHasSegments(text)).toBe(true);
  });
});
