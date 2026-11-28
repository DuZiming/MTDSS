import { describe, expect, it } from "vitest";

import { fmtDuration } from "./duration";

describe("fmtDuration(时长角标单点,T21 磁贴面)", () => {
  it("mm:ss:不满 1 小时,分不补零秒补零", () => {
    expect(fmtDuration(0)).toBe("");
    expect(fmtDuration(5)).toBe("0:05");
    expect(fmtDuration(63)).toBe("1:03");
    expect(fmtDuration(600)).toBe("10:00");
    expect(fmtDuration(3599.9)).toBe("59:59");
  });

  it("h:mm:ss:满 1 小时,时位不补零", () => {
    expect(fmtDuration(3600)).toBe("1:00:00");
    expect(fmtDuration(3723)).toBe("1:02:03");
    expect(fmtDuration(91.5)).toBe("1:31");
  });

  it("防御形:负数/非数 → 空串(角标不显,不落垃圾)", () => {
    expect(fmtDuration(-1)).toBe("");
    expect(fmtDuration(Number.NaN)).toBe("");
    expect(fmtDuration(Number.POSITIVE_INFINITY)).toBe("");
  });
});
