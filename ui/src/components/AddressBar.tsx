/// 地址栏:←/→/↑ 导航按钮(按历史栈/上级位启用)+ 当前视图面包屑 +
/// 搜索框(目录视图启用;过滤接线在 Task 8,本任务仅受控本地态)。
/// 面包屑:设备视图 = 「MTDSS设备」;目录视图 = MTDSS设备 ▸ 设备名 ▸
/// 路径段(pathSegments,点击经 locAtSegment 折叠导航),末段为当前位
/// 不可点。
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { locAtSegment, pathSegments, type Loc } from "../lib/nav";

/// 单个面包屑项:loc 为 null 表示即当前位(纯文本,不可点)。
interface Crumb {
  label: string;
  loc: Loc | null;
}

export default function AddressBar({
  current,
  canBack,
  canForward,
  canUp,
  onBack,
  onForward,
  onUp,
  onNavigate,
  deviceName,
}: {
  current: Loc;
  canBack: boolean;
  canForward: boolean;
  canUp: boolean;
  onBack(): void;
  onForward(): void;
  onUp(): void;
  onNavigate(loc: Loc): void;
  deviceName(deviceId: string): string;
}) {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");

  const crumbs: Crumb[] = [];
  if (current.kind === "devices") {
    crumbs.push({ label: t("sidebar.devices"), loc: null });
  } else {
    crumbs.push({ label: t("sidebar.devices"), loc: { kind: "devices" } });
    const segs = pathSegments(current.path);
    if (current.path === "") {
      // 设备根:设备 crumb 即当前位,不可点(否则自导航推重复历史)。
      crumbs.push({ label: deviceName(current.device), loc: null });
    } else {
      crumbs.push({
        label: deviceName(current.device),
        loc: { kind: "dir", device: current.device, path: "" },
      });
      segs.forEach((label, i) => {
        crumbs.push({ label, loc: locAtSegment(current.device, current.path, i + 1) });
      });
      // 末段即当前位:降为纯文本,不可点。
      crumbs[crumbs.length - 1].loc = null;
    }
  }

  return (
    <div className="addressbar">
      <button className="nav-btn" disabled={!canBack} onClick={onBack} title={t("address.back")}>
        ←
      </button>
      <button
        className="nav-btn"
        disabled={!canForward}
        onClick={onForward}
        title={t("address.forward")}
      >
        →
      </button>
      <button className="nav-btn" disabled={!canUp} onClick={onUp} title={t("address.up")}>
        ↑
      </button>
      <div className="breadcrumb">
        {crumbs.map((c, i) => (
          <span key={i} className="crumb-item">
            {i > 0 && <span className="crumb-sep">▸</span>}
            {c.loc == null ? (
              <span className="crumb-current">{c.label}</span>
            ) : (
              <button
                className="crumb-link"
                onClick={() => onNavigate(c.loc as Loc)}
                title={c.label}
              >
                {c.label}
              </button>
            )}
          </span>
        ))}
      </div>
      <input
        className="search-box"
        type="search"
        disabled={current.kind !== "dir"}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        placeholder={t("address.searchPlaceholder")}
      />
    </div>
  );
}
