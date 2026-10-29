/// 左侧树:「常用目录」灰占位(V2)+「MTDSS设备」可展开节点。
///
/// M3b 真实树:MTDSS设备 节点展开列出设备(sortDevices,本机置顶 +
/// 「(本机)」后缀);每台在线设备再展开即懒加载其根目录(路径空串),
/// 目录子节点逐级懒加载,子节点只显示目录(文件不可导航)。展开态与
/// 子目录缓存按 `device\0path` 键保存;失败节点给出瞬时错误样式,下次
/// 展开重试。当前导航位(Loc)高亮;离线设备连同其已展开的目录子节点
/// 灰显禁点禁展开(与主区 canEnterDir 门控同源)。
///
/// 拖放落点(Task 12):在线设备的**目录子节点**(设备根节点为卷列表,
/// path="" 不是合法传输目标)挂 `data-drop-loc="device\0path"` 供 App 的
/// 指针拖拽命中测试(elementFromPoint + closest);拖拽悬停高亮由
/// dropHoverKey 比对驱动(高亮态本身不参与命中,数据属性才是唯一依据)。
///
/// 视觉(P4b Task 7,spec §11):caret 换 SVG chevron(展开态 CSS 旋
/// 90°);设备节点=平台图标+名+在线点(--ok,仅在线设备渲染——离线行
/// 整体灰显,点略);目录子节点=16px 文件夹图标。「MTDSS设备」根行是
/// 分组标头,不带文件图标(裁定记档);侧栏独立滚动见 App.css .sidebar。
import { useCallback, useState, type MouseEvent } from "react";
import { useTranslation } from "react-i18next";

import { sortEntries, type FsEntry } from "../lib/browse";
import { zhCmp } from "../lib/collate";
import { daemonList } from "../lib/daemon";
import { sortDevices, type DeviceInfo } from "../lib/derived";
import type { Loc } from "../lib/nav";

import { IcChevronRight, IcFolder, IcPc, IcPhone } from "./icons";

/// 目录子节点缓存条目(懒加载结果;failed 时下次展开重试)。
interface DirChildren {
  loading: boolean;
  dirs: FsEntry[];
  failed: boolean;
}

/// 展开态/缓存键:device 与 path 以 NUL 分隔(路径不含 NUL,无歧义)。
function dirKey(device: string, path: string): string {
  return `${device}\u0000${path}`;
}

/// 拖放落点键:在线设备的目录子节点才可落(path="" 为卷列表,返回
/// undefined 即不挂 data-drop-loc,命中测试自然跳过)。
function dropTargetKey(device: string, path: string): string | undefined {
  return path === "" ? undefined : dirKey(device, path);
}

/// 缩进:每层 16px,基线 8px。
function indent(depth: number): { paddingLeft: string } {
  return { paddingLeft: `${8 + depth * 16}px` };
}

/// 平台图标(设备根节点,与 DeviceTable/AddressBar 同裁定):android →
/// 手机形;windows/linux/macos 及未知/缺失 → PC 形兜底。
function PlatformIcon({ platform }: { platform: string | undefined }) {
  return platform === "android" ? <IcPhone size={16} /> : <IcPc size={16} />;
}

/// 树节点(设备节点与目录子节点共用):caret 切换展开,label 点击导航。
/// 全部状态(展开集/缓存)由 Sidebar 持有,本组件无内部状态,可安全递归。
/// 禁用态由 deviceOnline 派生并沿递归传递:离线设备的根与其已展开的目录
/// 子节点一并灰显禁点禁展开(与主区 canEnterDir 门控同源)。
function DirNode(props: {
  device: string;
  path: string;
  name: string;
  depth: number;
  deviceOnline: boolean;
  /// 设备平台(仅设备根节点传,供平台图标;目录子节点不传,节点图标走
  /// IcFolder)。设备根节点以 path="" 识别(卷列表位,同 dropTargetKey)。
  platform?: string;
  current: Loc;
  expandedKeys: Set<string>;
  cache: Map<string, DirChildren>;
  /// 拖拽悬停高亮键(device\0path;null = 无悬停)。
  dropHoverKey: string | null;
  onToggle(device: string, path: string): void;
  onOpen(loc: Loc): void;
}) {
  const { t } = useTranslation();
  const disabled = !props.deviceOnline;
  const k = dirKey(props.device, props.path);
  const isOpen = !disabled && props.expandedKeys.has(k);
  const isCurrent =
    props.current.kind === "dir" &&
    props.current.device === props.device &&
    props.current.path === props.path;
  const child = props.cache.get(k);
  // 节点图标分支:设备根节点(path="")=平台图标+在线点;目录子节点=文件夹。
  const isDeviceNode = props.path === "";
  const cls =
    "tree-node" +
    (disabled ? " disabled" : " clickable") +
    (isCurrent ? " selected" : "") +
    (child?.failed ? " error" : "") +
    (dropTargetKey(props.device, props.path) === props.dropHoverKey ? " drop-hover" : "");
  const open = () => {
    if (!disabled) {
      props.onOpen({ kind: "dir", device: props.device, path: props.path });
    }
  };
  const toggle = (e: MouseEvent) => {
    e.stopPropagation();
    if (!disabled) props.onToggle(props.device, props.path);
  };
  // 拖放落点数据属性:仅在线设备的目录子节点(path="" 为卷列表,不是
  // 合法传输目标);命中测试与语义判读都在 App(App.css .drop-hover 高亮)。
  const dropLoc = dropTargetKey(props.device, props.path);
  return (
    <>
      <div
        className={cls}
        style={indent(props.depth)}
        onClick={open}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") open();
        }}
        role={disabled ? undefined : "button"}
        tabIndex={disabled ? undefined : 0}
        data-drop-loc={dropLoc ?? undefined}
      >
        <span className={"caret" + (isOpen ? " open" : "")} aria-hidden="true" onClick={toggle}>
          <IcChevronRight size={12} />
        </span>
        <span className="tree-icon" aria-hidden="true">
          {isDeviceNode ? <PlatformIcon platform={props.platform} /> : <IcFolder size={16} />}
        </span>
        <span className="tree-label">{props.name}</span>
        {isDeviceNode && props.deviceOnline && <span className="tree-dot" aria-hidden="true" />}
      </div>
      {isOpen && (
        <div role="group" className="tree-children">
          {child?.loading && (
            <div className="tree-status loading" style={indent(props.depth + 1)}>
              {/* 懒加载 spinner(Task 13 打磨):纯 CSS 旋转环,文案沿用「加载中…」 */}
              <span className="spinner" aria-hidden="true" />
              {t("sidebar.loading")}
            </div>
          )}
          {child?.failed && (
            <div className="tree-status error" style={indent(props.depth + 1)}>
              {t("sidebar.loadFailed")}
            </div>
          )}
          {child != null && !child.loading && !child.failed && child.dirs.length === 0 && (
            <div className="tree-status" style={indent(props.depth + 1)}>
              {t("sidebar.emptyDir")}
            </div>
          )}
          {child?.dirs.map((d) => (
            <DirNode
              key={d.path}
              device={props.device}
              path={d.path}
              name={d.name}
              depth={props.depth + 1}
              deviceOnline={props.deviceOnline}
              current={props.current}
              expandedKeys={props.expandedKeys}
              cache={props.cache}
              dropHoverKey={props.dropHoverKey}
              onToggle={props.onToggle}
              onOpen={props.onOpen}
            />
          ))}
        </div>
      )}
    </>
  );
}

export default function Sidebar({
  devices,
  selfDeviceId,
  current,
  dropHoverKey,
  onNavigate,
}: {
  devices: DeviceInfo[];
  selfDeviceId: string | null;
  current: Loc;
  /// 拖拽悬停高亮键(device\0path;null = 无悬停)。
  dropHoverKey: string | null;
  onNavigate(loc: Loc): void;
}) {
  const { t } = useTranslation();
  const [devicesOpen, setDevicesOpen] = useState(false);
  const [expandedKeys, setExpandedKeys] = useState<Set<string>>(new Set());
  const [cache, setCache] = useState<Map<string, DirChildren>>(new Map());

  /// 懒加载某设备某目录的目录子节点(过滤文件;真实 UI 用 zhCmp 排序)。
  const loadChildren = useCallback((device: string, path: string) => {
    const k = dirKey(device, path);
    setCache((m) => new Map(m).set(k, { loading: true, dirs: [], failed: false }));
    daemonList(device, path)
      .then((l) => {
        const dirs = sortEntries(l.entries, { key: "name", dir: "asc" }, zhCmp()).filter((e) => e.is_dir);
        setCache((m) => new Map(m).set(k, { loading: false, dirs, failed: false }));
      })
      .catch(() => {
        setCache((m) => new Map(m).set(k, { loading: false, dirs: [], failed: true }));
      });
  }, []);

  /// caret 切换展开:收起即回;展开时无缓存(或上次失败)则发起懒加载。
  const toggleNode = useCallback(
    (device: string, path: string) => {
      const k = dirKey(device, path);
      setExpandedKeys((s) => {
        const next = new Set(s);
        if (next.has(k)) {
          next.delete(k);
        } else {
          next.add(k);
        }
        return next;
      });
      const cur = cache.get(k);
      if (cur == null || cur.failed) loadChildren(device, path);
    },
    [cache, loadChildren],
  );

  const sorted = sortDevices(devices, selfDeviceId);
  const gotoDevices = () => onNavigate({ kind: "devices" });

  return (
    <div className="sidebar">
      <div className="tree-node disabled" title={t("sidebar.favoritesTag")}>
        <span className="caret" aria-hidden="true">
          <IcChevronRight size={12} />
        </span>
        <span className="tree-label">{t("sidebar.favorites")}</span>
        <span className="tree-tag">{t("sidebar.favoritesTag")}</span>
      </div>
      <div
        className={"tree-node clickable" + (current.kind === "devices" ? " selected" : "")}
        onClick={gotoDevices}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") gotoDevices();
        }}
        role="button"
        tabIndex={0}
      >
        <span
          className={"caret" + (devicesOpen ? " open" : "")}
          aria-hidden="true"
          onClick={(e) => {
            e.stopPropagation();
            setDevicesOpen((o) => !o);
          }}
        >
          <IcChevronRight size={12} />
        </span>
        <span className="tree-label strong">{t("sidebar.devices")}</span>
      </div>
      {devicesOpen &&
        sorted.map((d) => {
          const isSelf = selfDeviceId != null && d.device_id === selfDeviceId;
          return (
            <DirNode
              key={d.device_id}
              device={d.device_id}
              path=""
              name={(d.name ?? d.device_id) + (isSelf ? `(${t("devices.self")})` : "")}
              depth={1}
              deviceOnline={d.online}
              platform={d.platform}
              current={current}
              expandedKeys={expandedKeys}
              cache={cache}
              dropHoverKey={dropHoverKey}
              onToggle={toggleNode}
              onOpen={onNavigate}
            />
          );
        })}
    </div>
  );
}
