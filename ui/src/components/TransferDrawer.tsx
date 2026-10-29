/// 传输抽屉(P4b Task 8 重排,spec §10):窗底常驻的传输任务面。
///
/// 行为不变量零改动(spec §10:开合记忆/拖高/筛选四档/操作可用性矩阵/
/// 控制 RPC),重排呈现:
/// - **收起条 grid 八栏**:`折叠钮 | 类型徽章 | 任务名·对端→目标(1fr) |
///   进度条 150px | 百分比 48px | 速度 88px | 操作(max-content 靠右) |
///   计数徽章`。内容规则(用户裁定 2026-10-29):显示最新活动任务
///   (pickCollapsedJob:queued/running/paused 迭代序最后者);无活动=
///   最新终态任务(状态词占进度条+百分比位);完全无任务=「传输 · 暂无
///   传输任务」;计数徽章非零段「进行中 N · 中断 N · 失败 N」,全零整
///   徽章隐藏。整条点击开合,折叠钮/操作钮 stopPropagation 防误折叠。
/// - **展开表六列定比例**(table-layout:fixed):任务名 22% / 类型 9% /
///   方向 12% / 状态 34% / 传输速度 11% / 操作 12%(右对齐)。类型=三色
///   徽章(§4 语义色,本机复制/被动接收归「其他」现状保留);方向=图标+
///   文字靠左(IcDirRecv/Send/Swap 托盘语义,local_copy→swap、incoming→
///   recv、缺者 swap);状态=三段定宽跨行严格对齐(徽章 52px 文字居中 |
///   槽 160px | 百分比 38px 右对齐 tabular):进行中/已暂停=进度条布局
///   (暂停徽章已表意,进度照显);完成=--ok 满条+「100%」;失败/取消/
///   中断=原因文字左对齐占槽(错误串截断省略,无则状态词,--muted,红
///   语义由徽章承担);排队中=「排队中」文字。
/// - 开合/高度记忆在 UI 本地存储(`mtdss.drawer`,不进 daemon settings)。
///
/// 组件保持薄:状态收敛(事件 reducer/选任务/计数/速率/进度比/任务名组合)
/// 全在 lib/transfer.ts,控制副作用(RPC)由 App 注入回调。
import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  collapsedCounts,
  filterByKind,
  fmtSpeed,
  isTerminalState,
  jobDisplayName,
  pickCollapsedJob,
  progressRatio,
  type JobSnapUI,
  type KindFilter,
} from "../lib/transfer";

import {
  IcChevronDown,
  IcDirRecv,
  IcDirSend,
  IcDirSwap,
  IcPause,
  IcRedo,
  IcTrash,
  IcX,
} from "./icons";

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

/// 类型徽章三分类(spec §10):upload=发送;download 与被动 incoming=
/// 接收;remote_copy=调度;本机 local_copy/未知种类归「其他」(现状保留,
/// 不崩)。徽章色档见 App.css(--badge-* 语义 token)。
function badgeKind(kind: string): "upload" | "download" | "remote_copy" | "other" {
  if (kind === "upload") return "upload";
  if (kind === "download" || kind === "incoming") return "download";
  if (kind === "remote_copy") return "remote_copy";
  return "other";
}

/// 方向列图标(托盘语义,spec §10 存档 v4/v6 DIR 三形)。
const DIR_ICONS = { recv: IcDirRecv, send: IcDirSend, swap: IcDirSwap } as const;
type DirKey = keyof typeof DIR_ICONS;

/// kind → 方向列归并(spec §10):upload→send、download/incoming→recv、
/// remote_copy/local_copy→swap;类型徽章归「其他」的 kind 照流向映射,
/// 缺者 swap(方向列只表达 recv/send/swap 三托盘语义)。
function dirOf(kind: string): DirKey {
  switch (kind) {
    case "upload":
      return "send";
    case "download":
    case "incoming":
      return "recv";
    case "remote_copy":
    case "local_copy":
      return "swap";
    default:
      return "swap";
  }
}

/// 状态文案键(queued/running 归并「进行中」,既有口径;排队中的槽位
/// 文字另用 transfer.state.queued,见 DrawerRow 状态槽)。
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

/// 状态徽章语义色档(spec §4:进行中=accent 系、完成=--ok、失败=--err、
/// 中断/已取消=--muted 系;已暂停随进行中档——槽内进度条照显,徽章文字
/// 表意)。档名对应 App.css .sb-*。
function stateBadgeCls(state: string): "run" | "done" | "fail" | "int" {
  switch (state) {
    case "done":
      return "done";
    case "failed":
      return "fail";
    case "interrupted":
    case "cancelled":
      return "int";
    default:
      return "run";
  }
}

/// 操作可用性矩阵(spec §7.2,零改动):进行中=暂停(remote_copy 拒
/// pause 仅取消)·取消;已暂停=继续·取消;中断/失败=重新发起·清除;
/// 已取消=清除;完成=无。终态动作(重新发起/清除)只对有持久记录的
/// kind 开放(upload/download/remote_copy)——被动 incoming 与本机
/// local_copy 是 session 卡,daemon 侧无记录,终态后动作必然报「任务
/// 不存在」,不出按钮。
type RowAction = "pause" | "resume" | "cancel" | "reinitiate" | "clear";

/// 有持久记录的 kind(终态动作资格面,见 actionsFor 注)。
const RECORD_KINDS: ReadonlySet<string> = new Set([
  "upload",
  "download",
  "remote_copy",
]);

function actionsFor(job: JobSnapUI): RowAction[] {
  switch (job.state) {
    case "queued":
    case "running":
      return job.kind === "remote_copy" ? ["cancel"] : ["pause", "cancel"];
    case "paused":
      return ["resume", "cancel"];
    case "interrupted":
    case "failed":
      return RECORD_KINDS.has(job.kind) ? ["reinitiate", "clear"] : [];
    case "cancelled":
      return RECORD_KINDS.has(job.kind) ? ["clear"] : [];
    default:
      return [];
  }
}

/// 操作悬停提示文案(spec §7.2:暂停 · 继续 · 取消 · 重新发起 · 清除)。
const ACTION_LABEL: Record<RowAction, string> = {
  pause: "drawer.act.pause",
  resume: "drawer.act.resume",
  cancel: "drawer.act.cancel",
  reinitiate: "drawer.act.reinitiate",
  clear: "drawer.act.clear",
};

/// 操作图标(T3 IcPause/IcX/IcRedo/IcTrash,存档 v6 L 四形)。resume 无
/// 专属托盘形(icons.tsx 导出面被 T3 合同钉死,勿改),沿旧字形 ▶。
function ActionIcon({ action }: { action: RowAction }) {
  switch (action) {
    case "pause":
      return <IcPause />;
    case "resume":
      return (
        <span className="act-glyph" aria-hidden="true">
          ▶
        </span>
      );
    case "cancel":
      return <IcX />;
    case "reinitiate":
      return <IcRedo />;
    case "clear":
      return <IcTrash />;
  }
}

/// 状态槽内容三态判定面:进行中(queued/running/paused,进度条布局,
/// 排队中另有文字档)/ 完成(满条)/ 失败·取消·中断(原因文字)。
function isRunningLike(state: string): boolean {
  return state === "queued" || state === "running" || state === "paused";
}

/// 单行:六列渲染(定比例 fixed 布局),状态列三段定宽,操作列按可用性
/// 矩阵出按钮(在途置灰防连点)。
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
  const dir = dirOf(job.kind);
  const DirIc = DIR_ICONS[dir];
  const word = t(stateKey(job.state));
  const reasonLike =
    job.state === "failed" || job.state === "interrupted" || job.state === "cancelled";
  const run = (a: RowAction) => {
    if (a === "pause") onPause(job.id);
    else if (a === "resume") onResume(job.id);
    else if (a === "cancel") onCancel(job.id);
    else if (a === "reinitiate") onReinitiate(job.id);
    else onClear(job.id);
  };
  return (
    <tr data-job-id={job.id}>
      {/* 任务名:超界省略(fixed 列宽),悬停完整清单(src 为 daemon 拼接
          的源路径全集);中继兜底角标(v2-P1c)随名缀显 */}
      <td className="col-name" title={job.src}>
        {jobDisplayName(job)}
        {job.relay && <span className="drawer-relay">{t("transfer.relayBadge")}</span>}
      </td>
      <td className="col-kind">
        <span className={"badge b-" + badge}>{t(`drawer.badge.${badge}`)}</span>
      </td>
      {/* 方向:图标+文字靠左,托盘语义(spec §10);title=方向词 */}
      <td className="col-dir" title={t(`drawer.dir.${dir}`)}>
        <span className="dir-cell">
          <DirIc />
          {t(`drawer.dir.${dir}`)}
        </span>
      </td>
      {/* 状态:三段定宽跨行严格对齐——徽章 52px | 槽 160px | 百分比 38px */}
      <td className="col-state">
        <div className="stcell">
          <span className={"stbadge sb-" + stateBadgeCls(job.state)}>{word}</span>
          <div className="st-slot">
            {job.state === "queued" ? (
              // 排队中:文字占槽(尚无进度面)
              <span className="st-reason">{t("transfer.state.queued")}</span>
            ) : reasonLike ? (
              // 失败/取消/中断:原因文字左对齐占槽(错误串截断省略,无则
              // 状态词;颜色 --muted,红语义由徽章承担)
              <span className="st-reason" title={job.error ?? undefined}>
                {job.error || word}
              </span>
            ) : job.state === "done" ? (
              // 完成:--ok 满条 + 「100%」
              <div className="tbar full">
                <div className="tbar-fill" style={{ width: "100%" }} />
              </div>
            ) : (
              // 进行中/已暂停:进度条布局(ratio 未知 → 不确定态动画条)
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
            )}
          </div>
          <span className="st-pct">
            {job.state === "done"
              ? "100%"
              : isRunningLike(job.state)
                ? ratio == null
                  ? t("transfer.unknown")
                  : `${Math.round(ratio * 100)}%`
                : "—"}
          </span>
        </div>
      </td>
      {/* 传输速度:速率仅 running 渲染(暂停期/终态陈旧 speed_bps 不显示),
          其余「—」占位(存档 v6 形) */}
      <td className="col-speed">
        {job.state === "running" ? fmtSpeed(job.speed_bps) : "—"}
      </td>
      <td className="col-actions">
        {actionsFor(job).map((a) => (
          <button
            key={a}
            type="button"
            className="drawer-act"
            title={t(ACTION_LABEL[a])}
            aria-label={t(ACTION_LABEL[a])}
            disabled={busy}
            onClick={() => run(a)}
          >
            <ActionIcon action={a} />
          </button>
        ))}
      </td>
    </tr>
  );
}

/// 传输抽屉主体:收起条恒显(grid 八栏),展开区 = 筛选 chips + 六列表格
/// (内部滚动)。jobs/busy 由 App 持有,本组件只持 UI 本地面(开合/高度/
/// 筛选档)。
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
  /// 收起条内容(用户裁定,spec §10):全量作业面选最新活动/终态任务 +
  /// 三段计数(不随筛选档变化——收起条是聚合可视信号)。
  const collapsed = useMemo(() => pickCollapsedJob(jobs), [jobs]);
  const counts = useMemo(() => collapsedCounts(jobs), [jobs]);
  const collapsedRatio =
    collapsed != null ? progressRatio(collapsed.bytes_done, collapsed.bytes_total) : null;
  const collapsedTerminal = collapsed != null && isTerminalState(collapsed.state);
  const collapsedBadge = collapsed != null ? badgeKind(collapsed.kind) : null;
  /// 计数徽章非零段(零段隐藏,全零整徽章隐藏)。
  const countSegs = [
    { key: "active", on: counts.active > 0, label: "drawer.summaryActive", n: counts.active, warn: false },
    { key: "int", on: counts.interrupted > 0, label: "drawer.summaryInterrupted", n: counts.interrupted, warn: true },
    { key: "fail", on: counts.failed > 0, label: "drawer.summaryFailed", n: counts.failed, warn: true },
  ].filter((s) => s.on);

  const runAction = (a: RowAction, id: string) => {
    if (a === "pause") onPause(id);
    else if (a === "resume") onResume(id);
    else if (a === "cancel") onCancel(id);
    else if (a === "reinitiate") onReinitiate(id);
    else onClear(id);
  };

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
      {/* 收起条:grid 八栏(spec §10);整条点击开合,折叠钮/操作钮
          stopPropagation 防误折叠/误开合 */}
      <div className="drawer-summary" onClick={() => setOpen((v) => !v)}>
        <button
          type="button"
          className="drawer-fold"
          aria-expanded={open}
          aria-label={t("drawer.title")}
          title={t("drawer.title")}
          onClick={(e) => {
            e.stopPropagation();
            setOpen((v) => !v);
          }}
        >
          <span className={"fold-ic" + (open ? "" : " shut")} aria-hidden="true">
            <IcChevronDown />
          </span>
        </button>
        {collapsedBadge != null ? (
          <span className={"badge c-badge b-" + collapsedBadge}>
            {t(`drawer.badge.${collapsedBadge}`)}
          </span>
        ) : (
          <span className="c-badge" />
        )}
        {collapsed != null ? (
          <span className="c-name" title={`${collapsed.src} → ${collapsed.dst}`}>
            {jobDisplayName(collapsed)}
            {collapsed.relay && <span className="drawer-relay">{t("transfer.relayBadge")}</span>}
            <span className="c-sep">·</span>
            <span className="c-path">{collapsed.src || "…"}</span>
            <span className="c-arrow" aria-hidden="true">
              →
            </span>
            <span className="c-path">{collapsed.dst || "…"}</span>
          </span>
        ) : (
          // 完全无任务:「传输 · 暂无传输任务」(用户裁定)
          <span className="c-name is-empty">
            {t("drawer.title")}
            <span className="c-sep">·</span>
            {t("drawer.empty")}
          </span>
        )}
        {collapsed == null ? (
          // 无任务:条位/百分比位留空占位(grid 显式列,结构不漂移)
          <>
            <span className="c-bar" />
            <span className="c-pct" />
          </>
        ) : collapsedTerminal ? (
          // 终态任务:状态词占进度条+百分比位(span 两栏)
          <span className="c-state">{t(stateKey(collapsed.state))}</span>
        ) : (
          <>
            <span className="c-bar">
              <span className={"tbar" + (collapsedRatio == null ? " indet" : "")}>
                {collapsedRatio != null && (
                  <span className="tbar-fill" style={{ width: `${collapsedRatio * 100}%` }} />
                )}
              </span>
            </span>
            <span className="c-pct">
              {collapsedRatio == null ? t("transfer.unknown") : `${Math.round(collapsedRatio * 100)}%`}
            </span>
          </>
        )}
        <span className="c-rate">
          {collapsed == null
            ? ""
            : collapsed.state === "running"
              ? fmtSpeed(collapsed.speed_bps)
              : "—"}
        </span>
        <span className="c-acts">
          {collapsed != null &&
            actionsFor(collapsed).map((a) => (
              <button
                key={a}
                type="button"
                className="drawer-act"
                title={t(ACTION_LABEL[a])}
                aria-label={t(ACTION_LABEL[a])}
                disabled={busyIds.has(collapsed.id)}
                onClick={(e) => {
                  e.stopPropagation();
                  runAction(a, collapsed.id);
                }}
              >
                <ActionIcon action={a} />
              </button>
            ))}
        </span>
        {countSegs.length > 0 && (
          <span className="drawer-counts">
            {countSegs.map((s, i) => (
              <Fragment key={s.key}>
                {i > 0 && <span className="status-sep">·</span>}
                <span className={s.warn ? "warn" : undefined}>
                  {t(s.label, { count: s.n })}
                </span>
              </Fragment>
            ))}
          </span>
        )}
      </div>
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
