/// 传输卡片面板 + 通知回显(Task 13):
///
/// - **传输卡片**(窗内右下角堆叠,非独立 OS 窗):每个非终态作业一张卡
///   (方向字形/类型标签/src→dst 摘要/聚合进度条/文件计数/速率/暂停⇄继续
///   + 取消);终态卡 done 5s 自隐,failed/cancelled 留置并显示错误与
///   「可重新发起」提示,由用户关闭。显示容错(承接 daemon V1 已知面):
///   远端计数未知(files/bytes 总数 0)→ 进度走不确定态动画条 + 文案「…」;
///   速率只在 running 态显示(暂停期/终态的陈旧 speed_bps 不渲染)。
///   remote_copy 无本机数据面,pause 会被 daemon 拒——按钮照挂,错误经
///   toast 队列呈现(App 侧)。
/// - **通知回显**(NotifyToasts):event.notify 的窗内瞬时 toast(5s 自动
///   消隐,不堆叠——队列最多 3 条,超出丢最旧)。OS 系统通知由 daemon 侧
///   自发(notify-rust),此处**只做窗内回显**,不重复弹系统通知。
///
/// 组件保持薄:状态收敛(事件 reducer/速率/进度比)全在 lib/transfer.ts,
/// 控制副作用(pause/resume/cancel RPC)由 App 注入回调。
import { useEffect } from "react";
import { useTranslation } from "react-i18next";

import {
  fmtSpeed,
  isTerminalState,
  progressRatio,
  type JobSnapUI,
} from "../lib/transfer";

/// done 终态卡自隐延时(规格钉死 5s)。
const DONE_AUTO_HIDE_MS = 5000;

/// toast 展示时长(窗内回显,规格钉死 5s)。
const TOAST_TTL_MS = 5000;

/// 类型 → 方向字形(纯 unicode 几何字形,无 emoji,与树/列表图标语言一致):
/// upload ↑ 出件;download/incoming ↓ 收件;remote_copy ⇄ 双端直传;
/// local_copy ⇇ 本机盘内复制。
function kindGlyph(kind: string): string {
  switch (kind) {
    case "upload":
      return "↑";
    case "download":
    case "incoming":
      return "↓";
    case "remote_copy":
      return "⇄";
    case "local_copy":
      return "⇇";
    default:
      return "•";
  }
}

/// 单张卡片骨架(头/路径/进度条/元信息行的公共布局,终态/进行态各自填充)。
function CardShell(props: {
  job: JobSnapUI;
  tone: "active" | "done" | "failed";
  children: React.ReactNode;
  onClose: (() => void) | null;
}) {
  const { t } = useTranslation();
  return (
    <div className={"tcard " + props.tone} data-job-id={props.job.id}>
      <div className="tcard-head">
        <span className="tcard-dir" aria-hidden="true">
          {kindGlyph(props.job.kind)}
        </span>
        <span className="tcard-kind">{t(`transfer.kind.${props.job.kind}`)}</span>
        <span className={"tcard-state st-" + props.job.state}>
          {t(`transfer.state.${props.job.state}`)}
        </span>
        {props.onClose != null && (
          <button
            type="button"
            className="tcard-close"
            aria-label={t("transfer.dismiss")}
            onClick={props.onClose}
          >
            ×
          </button>
        )}
      </div>
      <div className="tcard-paths">
        <span className="tcard-path" title={props.job.src}>
          {props.job.src || "…"}
        </span>
        <span className="tcard-arrow" aria-hidden="true">
          →
        </span>
        <span className="tcard-path" title={props.job.dst}>
          {props.job.dst || "…"}
        </span>
      </div>
      {props.children}
    </div>
  );
}

/// 聚合进度条 + 计数元信息(进行态/终态共用):总量未知 → 不确定态动画条
/// + 「…」;已知 → 定宽填充 + 百分比。速率仅 running 态渲染(显示容错)。
function ProgressBlock({ job }: { job: JobSnapUI }) {
  const { t } = useTranslation();
  const ratio = progressRatio(job.bytes_done, job.bytes_total);
  const filesText =
    job.files_total > 0
      ? t("transfer.filesProgress", { done: job.files_done, total: job.files_total })
      : t("transfer.filesUnknown");
  const pct = ratio == null ? t("transfer.unknown") : `${Math.round(ratio * 100)}%`;
  return (
    <>
      <div
        className={"tbar" + (ratio == null ? " indet" : "")}
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={ratio == null ? undefined : Math.round(ratio * 100)}
      >
        {ratio != null && (
          <div className="tbar-fill" style={{ width: `${ratio * 100}%` }} />
        )}
      </div>
      <div className="tcard-meta">
        <span>{pct}</span>
        <span>{filesText}</span>
        {job.state === "running" && (
          <span className="tcard-speed">{fmtSpeed(job.speed_bps)}</span>
        )}
      </div>
    </>
  );
}

/// done 终态卡:5s 计时自隐(过期回调 onExpire 由 App 挪进隐藏集);计时
/// 挂在组件 effect 里,卡被隐藏/卸载即清,重复挂载(事件重放)自动重置。
function DoneCard({
  job,
  onExpire,
  onDismiss,
}: {
  job: JobSnapUI;
  onExpire(id: string): void;
  onDismiss(id: string): void;
}) {
  useEffect(() => {
    const timer = window.setTimeout(() => onExpire(job.id), DONE_AUTO_HIDE_MS);
    return () => window.clearTimeout(timer);
  }, [job.id, onExpire]);
  return (
    <CardShell job={job} tone="done" onClose={() => onDismiss(job.id)}>
      <ProgressBlock job={job} />
    </CardShell>
  );
}

/// 进行态卡:暂停⇄继续 + 取消(操作在途由 busyIds 置灰防连点;RPC 错误
/// 由 App 转 toast 呈现——remote_copy 的 pause 拒绝即走这条提示面)。
function ActiveCard({
  job,
  busy,
  onPause,
  onResume,
  onCancel,
}: {
  job: JobSnapUI;
  busy: boolean;
  onPause(id: string): void;
  onResume(id: string): void;
  onCancel(id: string): void;
}) {
  const { t } = useTranslation();
  const paused = job.state === "paused";
  return (
    <CardShell job={job} tone="active" onClose={null}>
      <ProgressBlock job={job} />
      <div className="tcard-actions">
        <button
          type="button"
          className="btn"
          disabled={busy}
          onClick={() => (paused ? onResume(job.id) : onPause(job.id))}
        >
          {paused ? t("transfer.resume") : t("transfer.pause")}
        </button>
        <button
          type="button"
          className="btn"
          disabled={busy}
          onClick={() => onCancel(job.id)}
        >
          {t("transfer.cancelBtn")}
        </button>
      </div>
    </CardShell>
  );
}

/// failed/cancelled 终态卡:留置 + 错误原文 + 「可重新发起」提示,用户
/// 关闭后消失(不自动隐——失败需要被看见,规格钉死)。
function FailedCard({
  job,
  onDismiss,
}: {
  job: JobSnapUI;
  onDismiss(id: string): void;
}) {
  const { t } = useTranslation();
  return (
    <CardShell job={job} tone="failed" onClose={() => onDismiss(job.id)}>
      <div className="tcard-err">{job.error || t("transfer.state.failed")}</div>
      <div className="tcard-hint">{t("transfer.retryHint")}</div>
    </CardShell>
  );
}

/// 卡片面板主体:过滤隐藏集后按作业 Map 首见序堆叠(非终态 + 终态留置/
/// 5s 自隐)。jobs/hidden 由 App 持有,本组件无状态(除 done 卡计时)。
export default function TransferCards({
  jobs,
  hidden,
  busyIds,
  onPause,
  onResume,
  onCancel,
  onDismiss,
  onExpire,
}: {
  jobs: ReadonlyMap<string, JobSnapUI>;
  hidden: ReadonlySet<string>;
  /// 控制操作在途的作业 id(按钮置灰防连点)。
  busyIds: ReadonlySet<string>;
  onPause(id: string): void;
  onResume(id: string): void;
  onCancel(id: string): void;
  onDismiss(id: string): void;
  onExpire(id: string): void;
}) {
  const cards: React.ReactNode[] = [];
  for (const job of jobs.values()) {
    if (hidden.has(job.id)) continue;
    if (!isTerminalState(job.state)) {
      cards.push(
        <ActiveCard
          key={job.id}
          job={job}
          busy={busyIds.has(job.id)}
          onPause={onPause}
          onResume={onResume}
          onCancel={onCancel}
        />,
      );
    } else if (job.state === "done") {
      cards.push(
        <DoneCard key={job.id} job={job} onExpire={onExpire} onDismiss={onDismiss} />,
      );
    } else {
      cards.push(<FailedCard key={job.id} job={job} onDismiss={onDismiss} />);
    }
  }
  if (cards.length === 0) return null;
  return (
    <div className="transfer-cards" role="region" aria-label="transfer-cards">
      {cards}
    </div>
  );
}

// ---------- 通知回显 toast(event.notify 的窗内面) ----------

/// 单条 toast 数据(id 由 App 侧单调分配,tone 区分信息/错误色)。
export interface ToastItem {
  id: number;
  title: string;
  body: string;
  tone: "info" | "error";
  /// 可选动作(Task 15):更新 toast 的「查看」直达更新页;点击不自动
  /// 出队(5s TTL 照常),动作闭包由 App 侧装配。
  action?: { label: string; onClick(): void };
}

/// 窗内 toast 栈:每条 5s 自隐(onExpire 挪出队列);不堆叠——App 侧入队
/// 时截尾最多 3 条。位置固定窗底中部(卡片在右下,互不遮挡)。
export function NotifyToasts({
  toasts,
  onExpire,
}: {
  toasts: readonly ToastItem[];
  onExpire(id: number): void;
}) {
  if (toasts.length === 0) return null;
  return (
    <div className="toasts" role="status">
      {toasts.map((t) => (
        <ToastRow key={t.id} toast={t} onExpire={onExpire} />
      ))}
    </div>
  );
}

/// 单条 toast:计时器挂自身 effect,卸载即清(队列截尾移除同样收干净)。
function ToastRow({
  toast,
  onExpire,
}: {
  toast: ToastItem;
  onExpire(id: number): void;
}) {
  useEffect(() => {
    const timer = window.setTimeout(() => onExpire(toast.id), TOAST_TTL_MS);
    return () => window.clearTimeout(timer);
  }, [toast.id, onExpire]);
  return (
    <div className={"toast" + (toast.tone === "error" ? " error" : "")}>
      <div className="toast-title">{toast.title}</div>
      <div className="toast-body">{toast.body}</div>
      {toast.action != null && (
        <button
          type="button"
          className="toast-action"
          onClick={toast.action.onClick}
        >
          {toast.action.label}
        </button>
      )}
    </div>
  );
}
