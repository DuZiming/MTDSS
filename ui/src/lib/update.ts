/// 更新纯逻辑层(Task 15):event.update 事件收敛 reducer / 状态文案映射 /
/// 安装门控 / 进度格式化。全部纯函数(不改入参、不触 IO),vitest 钉死
/// (`full_transition_table_idle_to_installing` /
/// `failed_carries_error_and_retains_it_against_late_progress` /
/// `stale_frames_after_ready_and_installing_are_discarded` 等见 update.test.ts)。
///
/// 数据面契约(与 daemon update.rs `state_json` 逐字对齐,事件帧与
/// `update.state` RPC 返回同形):
/// - `event.update` 帧 = {state, current, latest?, notes?, progress?, error?};
///   状态翻转即发,进度帧沿 jobs 口径 ≥400ms 节流(首条即发)。
/// - state:idle | available | downloading | ready | pending_install |
///   applying | installing | failed。`applying` 是 spec D11 的安装序列
///   事件名(实现现发 installing,CLI 契约两者同判)——UI 按同义词处理。
/// - 翻转帧不伪造进度(settle_downloading 恒 progress=null,进度只来自
///   真实字节记账);installing 是终态(daemon 不再离开,安装序列收尾即
///   退出);failed 保留 latest/notes(daemon fail() 不清,清单缓存仍可
///   用于 apply(None) 重试)。
///
/// reducer 丢弃规则(乱序旧帧防线;广播按序投递,风险面是重连重放与
/// 迟到节流帧,不是真乱序):
/// 1. installing 终态粘滞:其后任何帧整体忽略(新 daemon 实例由重拉
///    update.state 对齐,不走事件);
/// 2. 进度帧(progress != null 的 downloading)只属于进行中的下载:
///    落在 failed(保 error)/ready/pending_install 之后的进度帧 = 迟到
///    旧帧,丢弃。重试的新下载以翻转帧(progress=null)开局,不受影响;
/// 3. ready/applying 之后的 downloading 翻转帧丢弃(daemon 的 apply 自
///    ready/installing 幂等返回,不会再落 downloading);
/// 4. 检查入口对任意态都权威:available → idle(重查无新版)、failed →
///    available(重查得新版)等回退/恢复合法采纳,不做单调序假设。

/// 更新状态机状态(与 daemon update.rs 的状态串同源)。
export type UpdateState =
  | "idle"
  | "available"
  | "downloading"
  | "ready"
  | "pending_install"
  | "applying"
  | "installing"
  | "failed";

/// 线上事件帧 / update.state 返回(JSON 形;可选字段 = 线上可省略)。
export interface UpdateEvent {
  state: UpdateState;
  current: string;
  latest?: string | null;
  notes?: string | null;
  /// 0.0–1.0;仅 downloading 的节流进度帧携带,翻转帧恒省略/null。
  progress?: number | null;
  error?: string | null;
}

/// reducer 输出快照:可选字段全部归一为 null(组件侧免逐字段判 undefined)。
export interface UpdateSnapshot {
  state: UpdateState;
  current: string;
  latest: string | null;
  notes: string | null;
  progress: number | null;
  error: string | null;
}

/// 线上帧 → 归一快照:缺省字段补 null,progress 夹回 [0,1](防御:非有限
/// 数值/越界一律不可信;null/undefined = 无进度)。
function normalize(ev: UpdateEvent): UpdateSnapshot {
  const p =
    typeof ev.progress === "number" && Number.isFinite(ev.progress)
      ? Math.min(1, Math.max(0, ev.progress))
      : null;
  return {
    state: ev.state,
    current: ev.current ?? "",
    latest: ev.latest ?? null,
    notes: ev.notes ?? null,
    progress: p,
    error: ev.error ?? null,
  };
}

/// 乱序/迟到旧帧判定(规则见模块文档;只对「会撒谎」的帧说 stale——
/// 检查翻转帧永远权威)。
function isStaleFrame(prev: UpdateSnapshot, next: UpdateSnapshot): boolean {
  if (prev.state === "installing") return true; // 终态粘滞
  if (next.state === "downloading" && next.progress != null && prev.state !== "downloading") {
    return true; // 迟到进度帧(failed 保留 error / ready·pending 不回退)
  }
  if ((prev.state === "ready" || prev.state === "applying") && next.state === "downloading") {
    return true; // ready 后不会再落 downloading(apply 幂等返回)
  }
  return false;
}

/// 事件收敛 reducer:`updateStateFromEvents(prev, frame) -> 新快照`。
///
/// - 幂等:同值帧(事件重放/事件与 RPC 返回同形)返回原引用,React 侧
///   零重渲染;
/// - 乱序旧帧整体丢弃(规则见模块文档),同样原引用返回;
/// - 纯函数:不改写 prev。
export function updateStateFromEvents(
  prev: UpdateSnapshot | null,
  frame: UpdateEvent,
): UpdateSnapshot {
  const next = normalize(frame);
  if (
    prev != null &&
    prev.state === next.state &&
    prev.current === next.current &&
    prev.latest === next.latest &&
    prev.notes === next.notes &&
    prev.progress === next.progress &&
    prev.error === next.error
  ) {
    return prev; // 幂等:同帧重复
  }
  if (prev != null && isStaleFrame(prev, next)) return prev;
  return next;
}

/// 安装按钮可点状态(D4 中档 = 检查+提醒+手动确认,确认面即更新页:
/// available 必须可点,否则默认档「开+自动安装关」下手动安装不可达;
/// ready = 下载就绪瞬态;pending_install = 空闲门放行后立即安装)。
const INSTALLABLE_STATES: ReadonlySet<string> = new Set([
  "available",
  "ready",
  "pending_install",
]);

/// 安装按钮门控:仅 available/ready/pending_install 可点(纯决策面,
/// UpdatePage 与将来托盘/快捷面板复用同源)。
export function canInstall(st: UpdateSnapshot | UpdateState): boolean {
  const s = typeof st === "string" ? st : st.state;
  return INSTALLABLE_STATES.has(s);
}

/// 状态 → 文案 i18n key(文案留在 zh.json,纯函数不做翻译;调用方 t()
/// 后呈现)。applying 与 installing 同文案(spec D11 同义事件名)。
export function updateTierLabel(st: UpdateSnapshot | UpdateState): string {
  const s = typeof st === "string" ? st : st.state;
  switch (s) {
    case "idle":
      return "update.tier.idle";
    case "available":
      return "update.tier.available";
    case "downloading":
      return "update.tier.downloading";
    case "ready":
      return "update.tier.ready";
    case "pending_install":
      return "update.tier.pendingInstall";
    case "applying":
    case "installing":
      return "update.tier.installing";
    case "failed":
      return "update.tier.failed";
  }
}

/// 进度格式化(纯函数):null = 不确定态文案「…」;数值 → 整数百分比
/// (防御性夹回 [0,1] 后取整,normalize 已夹,此处兜底直调场景)。
export function formatProgress(progress: number | null): string {
  if (progress == null) return "…";
  const pct = Math.round(Math.min(1, Math.max(0, progress)) * 100);
  return `${pct}%`;
}
