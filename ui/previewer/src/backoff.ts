/// 503 seg_retry 退避纯函数(V4-1 T21;daemon T15/T15b 消费面):
/// 段等待超界 503 携 `seg_retry` 稳定词 + `Retry-After: {secs}` 头——
/// 播放端重试节律原料。本模块把节律钉成纯函数(可测单点):
/// - Retry-After 在场(>0)= 对齐服务端节律(×1000 毫秒);
/// - 缺席 = 指数退避 base×2^(attempt-1);
/// - 双路都封顶 CAP(~10s;与 daemon 单段有界等待窗同量级——服务端每窗
///   已含远跳重开换道,客户端退避只补请求节律,不宜再拉长)。
/// 消费面 = video.ts 错误环(重试中 spinner;attempt 在 FRAG_LOADED 清账)。

/// 退避封顶(毫秒;常量锚,改值须过测试)。
export const SEG_BACKOFF_CAP_MS = 10_000;
/// 指数退避起步(毫秒)。
export const SEG_BACKOFF_BASE_MS = 1_000;

/// 单次重试延迟(毫秒)。`retryAfterSecs` = Retry-After 头秒值(无效形
/// 0/负/NaN/null = 视同缺席);`attempt` 1 起步(非正防御为 1)。
export function segBackoffDelayMs(attempt: number, retryAfterSecs?: number | null): number {
  const n = typeof attempt === "number" && Number.isFinite(attempt) && attempt > 0 ? attempt : 1;
  const capped = (ms: number): number => Math.min(SEG_BACKOFF_CAP_MS, ms);
  if (retryAfterSecs != null && Number.isFinite(retryAfterSecs) && retryAfterSecs > 0) {
    return capped(retryAfterSecs * 1000);
  }
  return capped(SEG_BACKOFF_BASE_MS * 2 ** (n - 1));
}
