/// 传输抽屉(Task 9,spec §7):窗底常驻的传输任务面,右下浮卡
/// (TransferCards)退役。
///
/// - **收起(默认)**:细摘要条「传输 · 进行中 N · 速率 · 中断 N · 失败 N」,
///   点击开合;进行中 = 非终态计数、速率 = running 速度和(aggRate 同源);
///   中断/失败行数 > 0 以警示色提示。
/// - **展开**:默认 1/4 窗高,顶边把手可拖(1/4–3/4 窗高),列表内部滚动;
///   开合与高度记忆在 UI 本地存储(`mtdss.drawer`,不进 daemon settings)。
/// - **六列表格**(表头居中,spec §7.2):任务名(单文件=文件名,多文件/
///   目录=首项名+「· N 项」,悬停完整清单;中继兜底角标随名缀显)| 类型
///   (定宽三色文字徽章:发送蓝/接收绿/调度橙;被动 incoming 归「接收」)|
///   方向(源/箭头/目标三段定宽,箭头纵向对齐)| 状态(进行中/已暂停/
///   中断/失败/完成/已取消)| 传输速度(定宽进度条+「进度% · 速率」,
///   速率仅 running 渲染;失败行显示原因)| 操作(图标按钮左对齐,悬停
///   提示)。
/// - **操作可用性矩阵**(spec §7.2):进行中=暂停·取消(remote_copy 无
///   本机数据面拒 pause,仅取消);已暂停=继续·取消;中断/失败=重新发起·
///   清除;已取消=清除;完成=无。
/// - 列表面 = transfer.list 持久历史(live 快照合并),含中断/失败/完成
///   等历史行;筛选四档:全部/发送/接收/调度。
///
/// 组件保持薄:状态收敛(事件 reducer/速率/进度比/任务名组合)全在
/// lib/transfer.ts,控制副作用(RPC)由 App 注入回调。
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  aggRate,
  filterByKind,
  fmtSpeed,
  progressRatio,
  jobDisplayName,
  type JobSnapUI,
  type KindFilter,
} from "../lib/transfer";

/// 抽屉开合/高度记忆键(UI 本地存储,spec §7.1)。
const DRAWER_KEY = "mtdss.drawer";
/// 展开高度范围(窗高比):默认 1/4,把手拖 1/4–3/4(spec §7.1)。
const HEIGHT_MIN = 0.25;
const HEIGHT_MAX = 0.75;
const HEIGHT_DEFAULT = 0.25;

/// 筛选四档展示序(全部/发送/接收/调度)。
const KIND_FILTERS: readonly KindFilter[] = ["all", "upload", "download", "remote_copy"];

/// localStorage 记忆读取:损坏/缺省 → 收起 + 1/4(尽力而为,不影响功能)。
function readDrawerPrefs(): { open: boolean; height: number } {
  try {
    const raw = localStorage.getItem(DRAWER_KEY);
    if (raw != null) {
      const v = JSON.parse(raw) as { open?: unknown; height?: unknown };
      return {
        open: v.open === true,
        height: clampHeight(typeof v.height === "number" ? v.height : HEIGHT_DEFAULT),
      };
    }
  } catch {
    // 损坏按缺省(与 settings 损坏兜底同口径)
  }
  return { open: false, height: HEIGHT_DEFAULT };
}

function clampHeight(v: number): number {
  if (!(v > 0)) return HEIGHT_DEFAULT;
  return Math.min(HEIGHT_MAX, Math.max(HEIGHT_MIN, v));
}

/// 类型徽章三分类(spec §7.2):upload=发送;download 与被动 incoming=
/// 接收;remote_copy=调度;未知种类灰档兜底(不崩)。
function badgeKind(kind: string): "upload" | "download" | "remote_copy" | "other" {
  if (kind === "upload") return "upload";
  if (kind === "download" || kind === "incoming") return "download";
  if (kind === "remote_copy") return "remote_copy";
  return "other";
}

/// 徽章方向字形(纯 unicode 几何字形,与旧浮卡/树图标语言一致)。
function badgeGlyph(badge: string): string {
  switch (badge) {
    case "upload":
      return "↑";
    case "download":
      return "↓";
    case "remote_copy":
      return "⇄";
    default:
      return "•";
  }
}

/// 状态文案键(spec §7.2 六态:queued/running 归并「进行中」)。
function stateKey(state: string): string {
  switch (state) {
    case "paused":
      return "drawer.state.paused";
    case "interrupted":
      return "drawer.state.interrupted";
    case "failed":
      return "drawer.state.failed";
    case "done":
      return "drawer.state.done";
    case "cancelled":
      return "drawer.state.cancelled";
    default:
      return "drawer.state.running";
  }
}

/// 操作可用性矩阵(spec §7.2):进行中=暂停(remote_copy 拒 pause 仅
/// 取消)·取消;已暂停=继续·取消;中断/失败=重新发起·清除;已取消=
/// 清除;完成=无。
type RowAction = "pause" | "resume" | "cancel" | "reinitiate" | "clear";

function actionsFor(job: JobSnapUI): RowAction[] {
  switch (job.state) {
    case "queued":
    case "running":
      return job.kind === "remote_copy" ? ["cancel"] : ["pause", "cancel"];
    case "paused":
      return ["resume", "cancel"];
    case "interrupted":
    case "failed":
      return ["reinitiate", "clear"];
    case "cancelled":
      return ["clear"];
    default:
      return [];
  }
}

/// 操作图标与悬停提示文案(spec §7.2:⏸ 暂停 · ✕ 取消 · ↻ 重新发起 ·
/// 🗑 清除;继续为暂停的替位符)。
const ACTION_META: Record<RowAction, { icon: string; labelKey: string }> = {
  pause: { icon: "⏸", labelKey: "drawer.act.pause" },
  resume: { icon: "▶", labelKey: "drawer.act.resume" },
  cancel: { icon: "✕", labelKey: "drawer.act.cancel" },
  reinitiate: { icon: "↻", labelKey: "drawer.act.reinitiate" },
  clear: { icon: "🗑", labelKey: "drawer.act.clear" },
};

/// 单行:六列渲染,操作列按可用性矩阵出按钮(在途置灰防连点)。
function DrawerRow({
  job,
  busy,
  onPause,
  onResume,
  onCancel,
  onReinitiate,
  onClear,
}: {
  job: JobSnapUI;
  busy: boolean;
  onPause(id: string): void;
  onResume(id: string): void;
  onCancel(id: string): void;
  onReinitiate(id: string): void;
  onClear(id: string): void;
}) {
  const { t } = useTranslation();
  const ratio = progressRatio(job.bytes_done, job.bytes_total);
  const badge = badgeKind(job.kind);
  const failed = job.state === "failed";
  const run = (a: RowAction) => {
    if (a === "pause") onPause(job.id);
    else if (a === "resume") onResume(job.id);
    else if (a === "cancel") onCancel(job.id);
    else if (a === "reinitiate") onReinitiate(job.id);
    else onClear(job.id);
  };
  return (
    <tr data-job-id={job.id}>
      {/* 任务名:悬停完整清单(src 为 daemon 拼接的源路径全集);中继兜底
          角标( v2-P1c)随名缀显,信息性弱化小字 */}
      <td className="col-name" title={job.src}>
        {jobDisplayName(job)}
        {job.relay && <span className="drawer-relay">{t("transfer.relayBadge")}</span>}
      </td>
      <td className="col-kind">
        <span className={"badge b-" + badge}>
          <span className="badge-glyph" aria-hidden="true">
            {badgeGlyph(badge)}
          </span>
          {t(`drawer.badge.${badge}`)}
        </span>
      </td>
      {/* 方向:源/箭头/目标三段定宽,箭头纵向对齐(spec §7.2) */}
      <td className="col-dir">
        <span className="dir-seg" title={job.src}>
          {job.src || "…"}
        </span>
        <span className="dir-arrow" aria-hidden="true">
          →
        </span>
        <span className="dir-seg" title={job.dst}>
          {job.dst || "…"}
        </span>
      </td>
      <td className={"col-state st-" + job.state}>{t(stateKey(job.state))}</td>
      <td className="col-speed">
        {failed ? (
          // 失败行显示原因(如「N 项源已不存在…」,daemon 原文)
          <span className="speed-err" title={job.error ?? undefined}>
            {job.error || t("drawer.state.failed")}
          </span>
        ) : (
          <>
            {/* 定宽进度条:总量未知 → 不确定态动画条 + 「…」(既有口径) */}
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
            <div className="speed-meta">
              <span>
                {ratio == null ? t("transfer.unknown") : `${Math.round(ratio * 100)}%`}
              </span>
              {/* 速率仅 running 态渲染(暂停期/终态陈旧 speed_bps 不显示) */}
              {job.state === "running" && (
                <span className="speed-rate">{fmtSpeed(job.speed_bps)}</span>
              )}
            </div>
          </>
        )}
      </td>
      <td className="col-actions">
        {actionsFor(job).map((a) => (
          <button
            key={a}
            type="button"
            className="drawer-act"
            title={t(ACTION_META[a].labelKey)}
            aria-label={t(ACTION_META[a].labelKey)}
            disabled={busy}
            onClick={() => run(a)}
          >
            {ACTION_META[a].icon}
          </button>
        ))}
      </td>
    </tr>
  );
}

/// 传输抽屉主体:摘要条恒显,展开区 = 筛选 chips + 六列表格(内部滚动)。
/// jobs/busy 由 App 持有,本组件只持 UI 本地面(开合/高度/筛选档)。
export default function TransferDrawer({
  jobs,
  busyIds,
  onPause,
  onResume,
  onCancel,
  onReinitiate,
  onClear,
}: {
  jobs: ReadonlyMap<string, JobSnapUI>;
  /// 控制操作在途的作业 id(按钮置灰防连点)。
  busyIds: ReadonlySet<string>;
  onPause(id: string): void;
  onResume(id: string): void;
  onCancel(id: string): void;
  onReinitiate(id: string): void;
  onClear(id: string): void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(() => readDrawerPrefs().open);
  const [height, setHeight] = useState(() => readDrawerPrefs().height);
  const [filter, setFilter] = useState<KindFilter>("all");
  const rootRef = useRef<HTMLDivElement>(null);

  // 开合/高度记忆(UI 本地存储,spec §7.1;存储不可用只影响记忆)。
  useEffect(() => {
    try {
      localStorage.setItem(DRAWER_KEY, JSON.stringify({ open, height }));
    } catch {
      // 忽略:隐私模式/满额等,功能不受影响
    }
  }, [open, height]);

  const list = useMemo(() => [...jobs.values()], [jobs]);
  const shown = useMemo(() => filterByKind(list, filter), [list, filter]);
  const agg = useMemo(() => aggRate(list), [list]);
  const summary = useMemo(() => {
    let interrupted = 0;
    let failed = 0;
    for (const j of list) {
      if (j.state === "interrupted") interrupted += 1;
      else if (j.state === "failed") failed += 1;
    }
    return { interrupted, failed };
  }, [list]);

  /// 顶边把手拖高(1/4–3/4 窗高):按下时量一次抽屉底沿(状态栏高度等
  /// 布局细节一次吸收),move 期「底沿 − 指针 Y」现算窗高比,夹取后写入
  /// (千分位取整减少记忆写频)。把手 setPointerCapture(Chromium 捕获
  /// 语义保证拖出窗口后 move/up 仍送达,与主区指针拖拽同口径),窗口
  /// 失焦兜底取消;监听挂 window,收尾统一摘除。
  const onHandleDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    const el = rootRef.current;
    if (el == null) return;
    const bottom = el.getBoundingClientRect().bottom;
    try {
      (e.currentTarget as Element).setPointerCapture(e.pointerId);
    } catch {
      // 捕获失败(指针已不活动等):退化为普通监听,窗外事件可能丢,
      // blur 兜底收尾
    }
    const onMove = (ev: PointerEvent) => {
      const ratio = (bottom - ev.clientY) / window.innerHeight;
      setHeight(clampHeight(Math.round(ratio * 1000) / 1000));
    };
    const endDrag = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", endDrag);
      window.removeEventListener("pointercancel", endDrag);
      window.removeEventListener("blur", endDrag);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", endDrag);
    window.addEventListener("pointercancel", endDrag);
    window.addEventListener("blur", endDrag);
  };

  return (
    <div
      ref={rootRef}
      className={"drawer" + (open ? " open" : "")}
      style={open ? { height: `${height * 100}%` } : undefined}
    >
      {open && <div className="drawer-handle" onPointerDown={onHandleDown} aria-hidden="true" />}
      <button
        type="button"
        className="drawer-summary"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <span className="drawer-title">{t("drawer.title")}</span>
        <span className="status-sep">·</span>
        <span>{t("drawer.summaryActive", { count: agg.active })}</span>
        <span className="status-sep">·</span>
        <span className="drawer-rate">{fmtSpeed(agg.down_bps)}</span>
        <span className="status-sep">·</span>
        <span className={summary.interrupted > 0 ? "warn" : undefined}>
          {t("drawer.summaryInterrupted", { count: summary.interrupted })}
        </span>
        <span className="status-sep">·</span>
        <span className={summary.failed > 0 ? "warn" : undefined}>
          {t("drawer.summaryFailed", { count: summary.failed })}
        </span>
        <span className="drawer-caret" aria-hidden="true">
          {open ? "▾" : "▴"}
        </span>
      </button>
      {open && (
        <div className="drawer-body">
          <div className="drawer-chips" role="group" aria-label={t("drawer.filterLabel")}>
            {KIND_FILTERS.map((f) => (
              <button
                key={f}
                type="button"
                className={"chip" + (filter === f ? " on" : "")}
                onClick={() => setFilter(f)}
              >
                {t(`drawer.filter.${f}`)}
              </button>
            ))}
          </div>
          <div className="drawer-table-wrap">
            <table className="drawer-table">
              <thead>
                <tr>
                  <th className="col-name">{t("drawer.col.name")}</th>
                  <th className="col-kind">{t("drawer.col.kind")}</th>
                  <th className="col-dir">{t("drawer.col.direction")}</th>
                  <th className="col-state">{t("drawer.col.state")}</th>
                  <th className="col-speed">{t("drawer.col.speed")}</th>
                  <th className="col-actions">{t("drawer.col.actions")}</th>
                </tr>
              </thead>
              <tbody>
                {shown.map((j) => (
                  <DrawerRow
                    key={j.id}
                    job={j}
                    busy={busyIds.has(j.id)}
                    onPause={onPause}
                    onResume={onResume}
                    onCancel={onCancel}
                    onReinitiate={onReinitiate}
                    onClear={onClear}
                  />
                ))}
                {shown.length === 0 && (
                  <tr>
                    <td colSpan={6} className="empty">
                      {t("drawer.empty")}
                    </td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
}
