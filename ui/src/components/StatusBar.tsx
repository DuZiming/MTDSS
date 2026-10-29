/// 状态栏:目录视图 = 「{n} 个项目[,已选 m]」;设备视图 = 现行行为。
/// Task 13 增:目录加载态「加载中…」(browse.loading 透传)与传输速率
/// 「↓ 32 MB/s(N 个传输中)」(由 App 侧 live 作业 Map 经 aggRate 汇总
/// 下发,非状态栏自拉 RPC)。尾部恒为 信令:{未登录|离线|在线} · {N} 台设备。
import { useTranslation } from "react-i18next";

import { onlineText, type LoginStatus } from "../lib/derived";
import { fmtSpeed } from "../lib/transfer";

export default function StatusBar({
  status,
  deviceCount,
  itemCount,
  selectedCount,
  loading = false,
  transfer = null,
}: {
  status: LoginStatus | null;
  deviceCount: number;
  /// 目录视图条目数;null = 设备视图(不展示条目计数)。
  itemCount: number | null;
  selectedCount: number;
  /// 目录列表加载中(browse.loading;Task 13 加载态面)。
  loading?: boolean;
  /// 传输速率汇总(aggRate 输出);null 或 active=0 不展示速率段。
  transfer?: { down_bps: number; active: number } | null;
}) {
  const { t } = useTranslation();
  const text = status == null ? "…" : onlineText(status);
  const browsing = itemCount != null;
  return (
    <div className="statusbar">
      {browsing && (
        <>
          <span>
            {t("status.items", { count: itemCount ?? 0 })}
            {selectedCount > 0 ? t("status.selectedSuffix", { count: selectedCount }) : ""}
          </span>
          <span className="status-sep">·</span>
        </>
      )}
      {loading && (
        <>
          <span>{t("status.loading")}</span>
          <span className="status-sep">·</span>
        </>
      )}
      {transfer != null && transfer.active > 0 && (
        <>
          {/* 「↓ 32 MB/s(N 个传输中)」形态(规格钉死,全角括号);速率
              段 tabular 数字(P4b T8,spec §10) */}
          <span className="status-rate">
            ↓ {fmtSpeed(transfer.down_bps)}
            {t("status.transfers", { count: transfer.active })}
          </span>
          <span className="status-sep">·</span>
        </>
      )}
      <span>
        {t("status.signaling")}:{text}
      </span>
      <span className="status-sep">·</span>
      <span>{t("status.deviceCount", { count: deviceCount })}</span>
    </div>
  );
}
