/// 取数看门狗(r70g field #1):文档族取数(pdf 首窗/续 range、text 头窗/
/// 续窗、office 全量)此前**无超时**——远端 ctrl 拨号在切网/对端失联窗
/// 挂起时,fetch 连接建立后永无应答 = 页面恒黑/白无错误面(实机:回环
/// ESTABLISHED 挂死,加载圈转天荒地老)。本助手给每次取数 bounded:
/// 超时 = abort → 调用方既有 catch → 覆盖层 + 重试钮(用户可感知可重试,
/// 不再静默死等)。外层 destroy-abort(调用方自有 AbortController)经
/// 监听穿透合并。
///
/// 预算口径:45s——r42「用户等不及自退」族(300s 属媒体播放首帧窗;文档
/// 取数单窗 45s 已远超正常回环+远端读时延,再长即失联态,错误面更有用)。

/// 单次取数超时(毫秒;常量锚,改值须同步测试)。
export const FETCH_TIMEOUT_MS = 45_000;

/// fetch + 有界等待(超时 abort;外层 signal 穿透)。
export async function fetchWithTimeout(
  url: string,
  init: RequestInit = {},
  timeoutMs: number = FETCH_TIMEOUT_MS,
): Promise<Response> {
  const outer: AbortSignal | null | undefined = init.signal;
  const ctrl = new AbortController();
  const onOuterAbort = (): void => ctrl.abort();
  if (outer != null) outer.addEventListener("abort", onOuterAbort);
  const timer = setTimeout(() => ctrl.abort(), timeoutMs);
  try {
    return await fetch(url, { ...init, signal: ctrl.signal });
  } finally {
    clearTimeout(timer);
    if (outer != null) outer.removeEventListener("abort", onOuterAbort);
  }
}
