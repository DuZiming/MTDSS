/// 冲突对话框(Task 12 操作面,Windows 风格):传输前预扫出的同名冲突
/// 问询。四键(spec §4.2):跳过 / 覆盖;批量(≥2 条)时加 全部跳过 /
/// 全部覆盖——**全部键只作用剩余未决项**(已决项保持,见 lib/conflict 的
/// applyAllDecisions)。全部决完即自动放行(无单独「确定」键),Escape
/// 与遮罩点击 = 取消整次传输。目录对目录恒合并不进本对话框(预扫层已滤)。
///
/// 受控组件:conflicts 由调用方给定(不持有业务状态);决策表为本组件
/// 内部状态,open 翻转/冲突集变化时重置。
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { fmtMtime, fmtSize } from "../lib/browse";
import {
  applyAllDecisions,
  groupConflicts,
  policyFromDecisions,
  undecidedCount,
  type Conflict,
  type ConflictDecision,
  type DecisionMap,
} from "../lib/conflict";

/// 单侧描述:类型 + 大小(文件)+ 修改时间(目标侧且非 0 才给)。
function sideText(
  t: (k: string) => string,
  isDir: boolean,
  size: number,
  mtime?: number,
): string {
  const type = isDir ? t("files.type.dir") : t("files.type.file");
  if (isDir) return type;
  const sizePart = `${type},${fmtSize(size, false)}`;
  return mtime != null && mtime !== 0
    ? `${sizePart},${fmtMtime(mtime)}`
    : sizePart;
}

export default function ConflictDialog({
  open,
  conflicts,
  onResolve,
  onCancel,
}: {
  open: boolean;
  /// 本次预扫出的冲突行(open=true 期间视为同一批,变化即重置决策)。
  conflicts: Conflict[];
  /// 全部决完自动回调,携 skip/overwrite 名单(未决残留不会到达这里)。
  onResolve(policy: { skip: string[]; overwrite: string[] }): void;
  onCancel(): void;
}) {
  const { t } = useTranslation();
  const [decisions, setDecisions] = useState<DecisionMap>({});
  // 冲突集翻批即清决策(同一 open 会话内预扫结果变化属异常路径,重置最诚实)。
  useEffect(() => {
    setDecisions({});
  }, [open, conflicts]);

  const { single, rows } = groupConflicts(conflicts);
  const undecided = undecidedCount(rows, decisions);

  /// 单行决策:记入决策表;若这是最后一条未决,即刻落定放行。
  /// 决策表在 setState **外**计算(StrictMode 会双调 updater,副作用
  /// 必须留在渲染外——否则 onResolve 双发、传输起两次)。
  const decide = (rel: string, d: ConflictDecision) => {
    const next = { ...decisions, [rel]: d };
    setDecisions(next);
    if (undecidedCount(rows, next) === 0) {
      onResolve(policyFromDecisions(rows, next));
    }
  };

  /// 全部键:applyAllDecisions 只翻剩余未决项;翻完必有全决(空批不进
  /// 本对话框),即刻落定放行。
  const decideAll = (d: ConflictDecision) => {
    const next = applyAllDecisions(decisions, rows, d);
    setDecisions(next);
    onResolve(policyFromDecisions(rows, next));
  };

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onCancel]);

  if (!open) return null;

  return (
    <div className="dialog-mask" role="presentation" onClick={onCancel}>
      <div
        className="dialog conflict-dialog"
        role="alertdialog"
        aria-modal="true"
        aria-label={t("conflict.title")}
        onClick={(e) => e.stopPropagation()}
      >
        <p className="dialog-title">{t("conflict.title")}</p>
        <p className="dialog-body">
          {single
            ? t("conflict.singleBody", { name: rows[0]?.rel ?? "" })
            : t("conflict.batchBody", { count: rows.length })}
        </p>
        <div className="conflict-rows">
          {rows.map((c) => (
            <div key={c.rel} className="conflict-row">
              <div className="conflict-desc">
                <span className="conflict-name" title={c.rel}>
                  {c.rel}
                </span>
                <span className="conflict-meta">
                  {t("conflict.src")} {sideText(t, c.src_is_dir, c.src_size)}
                  {" · "}
                  {t("conflict.dst")}{" "}
                  {sideText(t, c.dst_is_dir, c.dst_size, c.dst_mtime_ms)}
                </span>
                {/* 类型不匹配(文件↔目录)单独提示:覆盖只清文件占用 */}
                {c.src_is_dir !== c.dst_is_dir && (
                  <span className="conflict-type-note">
                    {t("conflict.typeMismatch")}
                  </span>
                )}
              </div>
              {decisions[c.rel] == null ? (
                <div className="conflict-actions">
                  <button className="btn" onClick={() => decide(c.rel, "skip")}>
                    {t("conflict.skip")}
                  </button>
                  <button className="btn" onClick={() => decide(c.rel, "overwrite")}>
                    {t("conflict.overwrite")}
                  </button>
                </div>
              ) : (
                <span className="conflict-decided">
                  {decisions[c.rel] === "skip"
                    ? t("conflict.willSkip")
                    : t("conflict.willOverwrite")}
                </span>
              )}
            </div>
          ))}
        </div>
        <div className="dialog-actions">
          <button className="btn" onClick={onCancel}>
            {t("common.cancel")}
          </button>
          {!single && (
            <>
              <button className="btn" onClick={() => decideAll("skip")}>
                {t("conflict.skipAll", { count: undecided })}
              </button>
              <button className="btn primary" onClick={() => decideAll("overwrite")}>
                {t("conflict.overwriteAll", { count: undecided })}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
