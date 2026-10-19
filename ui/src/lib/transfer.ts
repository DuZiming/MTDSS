/// 传输作业纯逻辑层(Task 13):事件收敛 reducer / 速率格式化 / 状态栏
/// 汇总。全部纯函数(不改入参、不触 IO),vitest 钉死
/// (`apply_events_dedupes_and_orders` / `terminal_state_wins_over_late_progress`
/// / `fmt_speed_units` / `agg_rate_sums_active`)。
///
/// 数据面契约(与 daemon jobs.rs 逐字对齐):
/// - `event.transfer` 事件帧**不含 src/dst**(事件只带计数与状态),src/dst
///   只在 `transfer.list` 全量快照里——reducer 对缺省 src/dst 保留前值,
///   卡片种子(seeds)经 seedJobs 一次性带入。
/// - `error` 字段恒存在(null = 无错,非缺席判空)。
/// - 作业注册表终态不摘除(jobs 只增不减,V1 已知面):卡片可见性在
///   组件侧收敛(终态 done 5s 自隐 / failed-cancelled 留置可关),本层
///   只保证「终态粘滞」——迟到进度不得覆盖终态。

/// 作业状态:与 daemon jobs.rs 的状态串同源。
export type JobState = "queued" | "running" | "paused" | "done" | "failed" | "cancelled";

/// `event.transfer` 事件帧(emit_locked 字段面;无 src/dst)。
export interface TransferFrame {
  id: string;
  kind: string;
  state: string;
  files_done: number;
  files_total: number;
  bytes_done: number;
  bytes_total: number;
  speed_bps: number;
  /// 恒存在,null = 无错(非缺席)。
  error: string | null;
  /// 仅 transfer.list 种子帧携带;事件帧缺省时 reducer 保留前值。
  src?: string;
  dst?: string;
}

/// 卡片消费的 UI 快照:事件帧 + 种子补充的 src/dst。
export interface JobSnapUI extends TransferFrame {
  src: string;
  dst: string;
}

/// 终态集合(与 daemon `is_terminal_state` 同口径:粘滞,先到者胜)。
const TERMINAL_STATES: ReadonlySet<string> = new Set(["done", "failed", "cancelled"]);

/// 是否终态(粘滞语义的判定面:终态后迟到事件一律忽略)。
export function isTerminalState(state: string): boolean {
  return TERMINAL_STATES.has(state);
}

/// 事件收敛 reducer:`applyTransferEvents(prev, evt) -> 新 Map`。
///
/// - 按 id 去重合并:同 id 后者整体胜(最新快照),Map 首见序即卡片稳定
///   序(transfer.list 种子已按 id 升序,此后新作业追加,不随更新翻转)。
/// - 终态粘滞:prev 中该作业已终态 → 迟到进度/翻帧整体忽略(返回等价
///   副本;条目引用原样保留,组件 memo 面零成本)。
/// - 事件帧缺 src/dst(线上形态):保留 prev 值;全新 id 且无 src/dst →
///   空串兜底(卡片显示空路径,不崩)。
/// - 纯函数:不改写 prev。
export function applyTransferEvents(
  prev: ReadonlyMap<string, JobSnapUI>,
  evt: TransferFrame,
): Map<string, JobSnapUI> {
  const old = prev.get(evt.id);
  if (old != null && isTerminalState(old.state)) {
    // 终态粘滞:迟到的进度/翻帧不覆盖(与 daemon 侧同一语义,双保险)。
    return new Map(prev);
  }
  const next = new Map(prev);
  next.set(evt.id, {
    id: evt.id,
    kind: evt.kind ?? old?.kind ?? "upload",
    state: evt.state,
    files_done: evt.files_done,
    files_total: evt.files_total,
    bytes_done: evt.bytes_done,
    bytes_total: evt.bytes_total,
    speed_bps: evt.speed_bps,
    error: evt.error ?? null,
    src: evt.src ?? old?.src ?? "",
    dst: evt.dst ?? old?.dst ?? "",
  });
  return next;
}

/// transfer.list 全量快照 → 作业 Map(连接/重连种子):逐条过 reducer,
/// 与事件流同一收敛路径(终态粘滞同样生效)。App 侧把种子里的终态 id
/// 挪进隐藏集,历史终态卡不弹。
export function seedJobs(
  snaps: readonly JobSnapUI[],
): Map<string, JobSnapUI> {
  return snaps.reduce<Map<string, JobSnapUI>>(
    (m, s) => applyTransferEvents(m, s),
    new Map(),
  );
}

/// 速率格式化(bps → 人类可读,纯函数):0 → `0 B/s`;<1024 → 整数字节
/// `N B/s`;此后 KB/s / MB/s / GB/s 进制 1024,恒一位小数(`1.2 MB/s` 形态)。
/// 显示容错:入参来自 daemon 的 u64,恒非负;异常输入按 0 兜底。
export function fmtSpeed(bps: number): string {
  if (!(bps > 0)) return "0 B/s";
  if (bps < 1024) return `${Math.round(bps)} B/s`;
  const UNITS = ["KB", "MB", "GB", "TB"] as const;
  let v = bps / 1024;
  let u = 0;
  while (v >= 1024 && u < UNITS.length - 1) {
    v /= 1024;
    u += 1;
  }
  return `${v.toFixed(1)} ${UNITS[u]}/s`;
}

/// 状态栏速率汇总:down_bps = **running 态**作业速度和(暂停期陈旧值、
/// 终态残留值不计入,显示容错);active = 非终态作业数(排队/运行/暂停
/// 都算「传输中」,与托盘 tooltip 计数同口径)。
export function aggRate(jobs: Iterable<JobSnapUI>): {
  down_bps: number;
  active: number;
} {
  let down_bps = 0;
  let active = 0;
  for (const j of jobs) {
    if (isTerminalState(j.state)) continue;
    active += 1;
    if (j.state === "running") down_bps += j.speed_bps;
  }
  return { down_bps, active };
}

/// 聚合进度比(卡片进度条):bytes_total 已知(>0)→ done/total 夹在
/// [0,1];总量未知(remote_copy/download/incoming 的远端计数面,以及部分
/// 作业起步前)→ null = 不确定态(卡片画动画条,文案显示「…」)。
export function progressRatio(bytes_done: number, bytes_total: number): number | null {
  if (bytes_total <= 0) return null;
  return Math.min(1, Math.max(0, bytes_done / bytes_total));
}
