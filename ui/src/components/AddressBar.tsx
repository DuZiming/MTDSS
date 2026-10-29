/// 地址栏(P4b Task 4 重设计,spec §7):←/→/↑ SVG 导航按钮(禁用灰)+
/// **设备 chip**(浏览期间恒定:平台图标+设备名+chevron,devbg 药丸;
/// 设备层呈「设备列表」灰态;点击弹全部设备下拉——在线可选、离线灰显
/// 禁选、当前项勾选,选中=进入该设备根并入导航历史)+ 面包屑(现有段
/// 逻辑原样:pathSegments 数据源、locAtSegment 点击导航、data-drop-loc
/// 拖放落点、末段粗体不可点)+ 复制路径(纯图标;设备层与卷列表位
/// path="" 均禁用,写入经 App 的 onCopyPath)+ 搜索框(行为不变)。
///
/// 编辑态:点面包屑空白处 / F4 / Ctrl+L 进入(modalBlocked——App 侧
/// modalBusy 判定结果以 prop 传入——时不进入,输入框聚焦时不劫持快捷键;
/// **设备层同样可进**:预填空串,提交以 current=null 参与解析——带设备
/// 前缀输入直达目标,无前缀输入得 needDeviceOrAbsolute 由 App toast,
/// §7 规则 2 的该分支即在此触发),crumbbox 整体变为输入框,预填当前
/// 完整原生路径并全选;**Enter = parseAddressInput 解析(纯函数在 lib/
/// address.ts)交 onAddressSubmit(App 转 navigate/toast);Esc 或失焦 =
/// 还原面包屑**;空/纯空白输入回车视同取消(不发解析)。编辑期间面包屑
/// 不在场,拖放落点随之不在(输入框优先,spec §7)。
///
/// 设备下拉:crumbbox 内绝对定位(crumbbox 恒 overflow 可见——长路径截
/// 断靠逐段省略,不裁下拉);外点/Esc/导航换位收起;编辑进入时强制收起。
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  IcBack,
  IcCheck,
  IcChevronDown,
  IcChevronRight,
  IcCopyPath,
  IcForward,
  IcPc,
  IcPhone,
  IcSearch,
  IcUp,
} from "./icons";
import { parseAddressInput, type AddressTarget } from "../lib/address";
import type { DeviceInfo } from "../lib/derived";
import { locAtSegment, pathSegments, type Loc } from "../lib/nav";

/// 平台图标:android → 手机形,其余(windows/linux/macos/未知)→ PC 形。
function PlatformIcon({ platform, size = 14 }: { platform: string | undefined; size?: number }) {
  return platform === "android" ? <IcPhone size={size} /> : <IcPc size={size} />;
}

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
  searchQ,
  onSearchChange,
  dropHoverKey,
  devices,
  onAddressSubmit,
  onCopyPath,
  modalBlocked,
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
  /// 当前搜索词(App 持有,换目录清空)。
  searchQ: string;
  onSearchChange(q: string): void;
  /// 拖拽悬停高亮键(device\0path;null = 无悬停)。
  dropHoverKey: string | null;
  /// 设备全量快照(chip 下拉数据源 + 编辑态解析的设备表)。
  devices: DeviceInfo[];
  /// 编辑态回车解析产物上交(App 转 navigate / toast)。
  onAddressSubmit(target: AddressTarget): void;
  /// 复制当前目录原生完整路径(App 经 daemonCopyText 写剪贴板+toast)。
  onCopyPath(): void;
  /// 模态占线(App 侧 modalBusy 判定结果):占线时不进编辑态。
  modalBlocked: boolean;
}) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [devOpen, setDevOpen] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const crumbboxRef = useRef<HTMLDivElement>(null);

  const isDir = current.kind === "dir";

  /// 面包屑段的拖放落点键:目录位且非卷列表段才可落(device\0path)。
  const dropKey = (loc: Loc | null): string | undefined =>
    loc != null && loc.kind === "dir" && loc.path !== ""
      ? `${loc.device}\u0000${loc.path}`
      : undefined;

  /// 进入编辑态:预填当前完整原生路径(目录位;设备层/卷列表位预填空串)
  /// 并全选(effect 见下);模态占线或已在编辑时不进入,**设备层同样可进**
  /// (§7 规则 2 的 needDeviceOrAbsolute 分支即由设备层编辑触发);进入
  /// 同时收起设备下拉。
  const beginEdit = useCallback(() => {
    if (modalBlocked || editing) return;
    setDevOpen(false);
    setDraft(current.kind === "dir" ? current.path : "");
    setEditing(true);
  }, [current, modalBlocked, editing]);

  /// F4 / Ctrl+L 全局快捷键:非模态占线、未在编辑才进入(设备层亦可);
  /// 输入框聚焦时不劫持(含编辑态自身输入框——再按不重置草稿)。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const k = e.key.toLowerCase();
      if (e.key !== "F4" && !((e.ctrlKey || e.metaKey) && !e.altKey && k === "l")) {
        return;
      }
      const tgt = e.target as HTMLElement | null;
      if (
        tgt != null &&
        (tgt.tagName === "INPUT" || tgt.tagName === "TEXTAREA" || tgt.isContentEditable)
      ) {
        return;
      }
      if (modalBlocked || editing) return;
      e.preventDefault();
      beginEdit();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [modalBlocked, editing, beginEdit]);

  /// 编辑态挂载即聚焦并全选(预填路径整段选中,直接输入即整段替换)。
  useEffect(() => {
    if (!editing) return;
    inputRef.current?.focus();
    inputRef.current?.select();
  }, [editing]);

  /// 导航换位:编辑态还原、设备下拉收起(导航必换 current 对象身份)。
  useEffect(() => {
    setEditing(false);
    setDevOpen(false);
  }, [current]);

  /// 设备下拉的外点/Esc 收起(下拉打开期间挂 document 级监听)。
  useEffect(() => {
    if (!devOpen) return;
    const onDown = (e: PointerEvent) => {
      if (crumbboxRef.current?.contains(e.target as Node)) return;
      setDevOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setDevOpen(false);
    };
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [devOpen]);

  /// 编辑态回车提交:空/纯空白视同取消(还原,不发解析——文档化裁定);
  /// 否则 parseAddressInput(纯函数,lib/address.ts)解析后上交并还原。
  /// 设备层以 current=null 参与解析:带设备前缀直达目标,无前缀(相对或
  /// 绝对形)得 needDeviceOrAbsolute 由 App toast(spec §7 规则 2)。
  const submitDraft = () => {
    const s = draft.trim();
    setEditing(false);
    if (s === "") return;
    onAddressSubmit(
      parseAddressInput(
        s,
        devices,
        current.kind === "dir" ? { device: current.device, path: current.path } : null,
      ),
    );
  };

  /// 面包屑空白处点击进编辑态:仅 crumbbox 本体或弹簧空隙(段/芯片/
  /// 下拉的点击各走各的语义)。
  const onCrumbboxClick = (e: React.MouseEvent<HTMLDivElement>) => {
    const el = e.target as Element;
    if (el === e.currentTarget || el.classList.contains("crumb-spring")) beginEdit();
  };

  /// 下拉选中设备 = 进该设备根(入导航历史);离线禁选(按钮已 disabled,
  /// 此为防御)。
  const pickDevice = (d: DeviceInfo) => {
    setDevOpen(false);
    if (!d.online) return;
    onNavigate({ kind: "dir", device: d.device_id, path: "" });
  };

  /// 面包屑段数据(现有逻辑原样):设备视图 = 「MTDSS设备」;目录视图 =
  /// MTDSS设备 ▸ 设备名 ▸ 路径段,末段为当前位不可点。
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

  /// 当前浏览设备快照(chip 平台图标/下拉勾选;设备层或快照缺失为
  /// undefined,图标落 PC 形缺省)。
  const curDevice =
    current.kind === "dir"
      ? devices.find((d) => d.device_id === current.device)
      : undefined;

  /// 复制路径禁用面:设备层与卷列表位(path="",无原生路径可复制)。
  const copyDisabled = !isDir || current.path === "";

  return (
    <div className="addressbar">
      <button
        type="button"
        className="nav-btn"
        disabled={!canBack}
        onClick={onBack}
        title={t("address.back")}
        aria-label={t("address.back")}
      >
        <IcBack />
      </button>
      <button
        type="button"
        className="nav-btn"
        disabled={!canForward}
        onClick={onForward}
        title={t("address.forward")}
        aria-label={t("address.forward")}
      >
        <IcForward />
      </button>
      <button
        type="button"
        className="nav-btn"
        disabled={!canUp}
        onClick={onUp}
        title={t("address.up")}
        aria-label={t("address.up")}
      >
        <IcUp />
      </button>
      <div className="crumbbox" ref={crumbboxRef} onClick={onCrumbboxClick}>
        {editing ? (
          <input
            ref={inputRef}
            className="addr-input"
            value={draft}
            spellCheck={false}
            placeholder={
              current.kind === "dir" ? undefined : t("address.needDeviceOrAbsolute")
            }
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                submitDraft();
              } else if (e.key === "Escape") {
                e.preventDefault();
                setEditing(false); // Esc = 还原面包屑
              }
            }}
            onBlur={() => setEditing(false)} // 失焦 = 还原
          />
        ) : (
          <>
            <button
              type="button"
              className={
                "devchip" + (current.kind === "devices" ? " devchip-empty" : "")
              }
              onClick={() => setDevOpen((o) => !o)}
              title={
                isDir ? deviceName(current.device) : t("address.deviceList")
              }
              aria-expanded={devOpen}
            >
              <PlatformIcon platform={curDevice?.platform} />
              <span className="devchip-name">
                {isDir ? deviceName(current.device) : t("address.deviceList")}
              </span>
              <span className="devchip-chev">
                <IcChevronDown />
              </span>
            </button>
            {crumbs.map((c, i) => (
              <span key={i} className="crumb-item">
                {i > 0 && (
                  <span className="csep">
                    <IcChevronRight size={11} />
                  </span>
                )}
                {c.loc == null ? (
                  <span className="crumb cur">{c.label}</span>
                ) : (
                  <button
                    type="button"
                    className={
                      "crumb crumb-link" +
                      (dropKey(c.loc) === dropHoverKey ? " drop-hover" : "")
                    }
                    onClick={() => onNavigate(c.loc as Loc)}
                    title={c.label}
                    data-drop-loc={dropKey(c.loc)}
                  >
                    {c.label}
                  </button>
                )}
              </span>
            ))}
            <span className="crumb-spring" />
          </>
        )}
        {devOpen && !editing && (
          <div className="devmenu" role="menu">
            {devices.length === 0 && (
              <div className="devmenu-item devmenu-empty">{t("devices.empty")}</div>
            )}
            {devices.map((d) => (
              <button
                key={d.device_id}
                type="button"
                className="devmenu-item"
                disabled={!d.online}
                onClick={() => pickDevice(d)}
                title={d.online ? d.name ?? d.device_id : t("devices.offline")}
              >
                <span className="devmenu-chk">
                  {current.kind === "dir" && current.device === d.device_id ? (
                    <IcCheck size={12} />
                  ) : null}
                </span>
                <PlatformIcon platform={d.platform} />
                <span className="devmenu-name">{d.name ?? d.device_id}</span>
                {d.online && <span className="devmenu-dot" aria-hidden="true" />}
              </button>
            ))}
          </div>
        )}
      </div>
      <button
        type="button"
        className="nav-btn"
        disabled={copyDisabled}
        onClick={onCopyPath}
        title={t("address.copyPath")}
        aria-label={t("address.copyPath")}
      >
        <IcCopyPath />
      </button>
      <span className="searchbox">
        <IcSearch />
        <input
          className="search-input"
          type="search"
          disabled={!isDir}
          value={searchQ}
          onChange={(e) => onSearchChange(e.target.value)}
          placeholder={t("address.searchPlaceholder")}
          aria-label={t("address.searchPlaceholder")}
        />
      </span>
    </div>
  );
}
