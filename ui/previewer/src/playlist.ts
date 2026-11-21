/// HLS playlist 纯解析(T15-M7 转达钉面的单点):
/// daemon 首刷 = init-only 瞬态零段 playlist(仅 attach/init,pulled≤1 = 纯头
/// 形,无 EXTINF)——消费形态钉死:**首刷纯头 → 按 TARGETDURATION 轮询重载,
/// 直到首段出现或超时**。本模块只出判别/节律两个纯函数;轮询环在 video.ts
/// (代次守卫/重试面/超时账)。

/// TARGETDURATION 秒解析:`#EXT-X-TARGETDURATION:` 首个在册值;缺行/非数
/// 回退 daemon 生产缺省 4(段长 4000ms);在册但 <1 钳最小 1(轮询环零除
/// /零延迟防护)。
export function playlistTargetDurationSecs(text: string): number {
  const m = /^#EXT-X-TARGETDURATION:(\S+)/m.exec(text);
  const n = m === null ? Number.NaN : Number(m[1]);
  if (!Number.isFinite(n)) return 4;
  return Math.max(1, Math.floor(n));
}

/// 是否已有内容段(EXTINF 在场;init 段的 EXT-X-MAP 不算)。
export function playlistHasSegments(text: string): boolean {
  return /(^|\n)#EXTINF/.test(text);
}
