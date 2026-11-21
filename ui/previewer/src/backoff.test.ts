import { describe, expect, it } from "vitest";

import { SEG_BACKOFF_BASE_MS, SEG_BACKOFF_CAP_MS, segBackoffDelayMs } from "./backoff";

describe("segBackoffDelayMs(503 seg_retry 退避纯函数,T15-M5/T15b 消费面)", () => {
  it("Retry-After 在场 = 对齐服务端节律(毫秒换算,封顶 CAP)", () => {
    expect(segBackoffDelayMs(1, 2)).toBe(2000);
    expect(segBackoffDelayMs(5, 3)).toBe(3000);
    expect(segBackoffDelayMs(1, SEG_RETRY_AFTER_ANCHOR)).toBe(SEG_RETRY_AFTER_ANCHOR * 1000);
  });

  it("Retry-After 缺席 = 指数退避(base×2^(attempt-1))", () => {
    expect(segBackoffDelayMs(1)).toBe(SEG_BACKOFF_BASE_MS);
    expect(segBackoffDelayMs(2)).toBe(SEG_BACKOFF_BASE_MS * 2);
    expect(segBackoffDelayMs(3)).toBe(SEG_BACKOFF_BASE_MS * 4);
    expect(segBackoffDelayMs(4)).toBe(SEG_BACKOFF_BASE_MS * 8);
  });

  it("双路都封顶 ~10s(CAP 常量锚,改值须过测试)", () => {
    expect(SEG_BACKOFF_CAP_MS).toBe(10_000);
    expect(segBackoffDelayMs(10)).toBe(SEG_BACKOFF_CAP_MS);
    expect(segBackoffDelayMs(99)).toBe(SEG_BACKOFF_CAP_MS);
    expect(segBackoffDelayMs(1, 3600)).toBe(SEG_BACKOFF_CAP_MS);
  });

  it("Retry-After 无效形(0/负/null/非数)= 视同缺席走指数臂", () => {
    expect(segBackoffDelayMs(1, 0)).toBe(SEG_BACKOFF_BASE_MS);
    expect(segBackoffDelayMs(2, -3)).toBe(SEG_BACKOFF_BASE_MS * 2);
    expect(segBackoffDelayMs(3, null)).toBe(SEG_BACKOFF_BASE_MS * 4);
    expect(segBackoffDelayMs(3, Number.NaN)).toBe(SEG_BACKOFF_BASE_MS * 4);
  });

  it("attempt 非正数 = 按 1 起步(防御形,不产 0/负延迟)", () => {
    expect(segBackoffDelayMs(0)).toBe(SEG_BACKOFF_BASE_MS);
    expect(segBackoffDelayMs(-5)).toBe(SEG_BACKOFF_BASE_MS);
  });
});

/// daemon SEG_RETRY_AFTER_SECS=2 的对齐锚(测试局部常量,与常量锚测试同义)。
const SEG_RETRY_AFTER_ANCHOR = 2;
