/// 左侧树:「快捷目录」区(V3-3 §5.2 点亮;返工 §13.1 树形化)+
/// 「Synshu 设备」可展开节点。
///
/// 快捷区(返工 §13.1,2026-11-06 钦定):「快捷目录」组节点=与「Synshu
/// 设备」组同款可展开 caret,默认展开(裁定:组开即收件箱+快捷条目+成员
/// 目录节点全部可见)→ 置顶收件箱行(本机收件目录快捷位)→ 快捷条目行
/// (FavNode):行本体点击=开常用视图(fav Loc,混合视图);行 caret=
/// 展开/收起成员目录节点——每成员一枚 DirNode(depth+1),完全复刻设备
/// 树语义:懒加载子目录(daemonList(member.device, member.path) 起,子级
/// 逐级)、点击导航、拖放落点 `device\0path`、离线成员连同其子树灰显禁点
/// 禁展开(在线态从 devices 快照 join,快照缺该设备按离线)。成员节点及
/// 其懒加载子目录节点点击=onOpenFavDir(favId, device, clickedPath)——
/// V3-3 返工 §13.6 起进快捷命名空间(favdir Loc;App 侧经 favdirAt 归一
/// 为 base=成员路径+rel=相对段,任何深度同规范形态),树结构不动。右键=
/// onFavContextMenu
/// (id, x, y)(T13:App 据此开 TreeNodeMenu——打开/重命名/管理成员/
/// 删除常用)保留;右端「+」新建钮已撤(§13.2,R5 由左上「新建」下拉
/// 承接)。展开键共用一套 expandedKeys:目录键 `device\0path`,常用行键
/// `fav\0id`(无碰撞;常用行取反解释——缺省展开,用户收起才入集)。
/// 拖放落点(Task 12 消费):收件箱行 `device\0recv_dir`、快捷行
/// `fav\0id` 保持不变;成员行落点由 DirNode 自挂。showFavorites=false
/// (未登录)整节不渲染(含标头),未登录面与 V2 一致=零回归。
///
/// M3b 真实树:「Synshu 设备」节点展开列出设备(sortDevices,本机置顶 +
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
/// 整体灰显,点略);目录子节点=16px 文件夹图标。「Synshu 设备」「快捷
/// 目录」两根行是分组标头,不带文件图标(裁定记档);侧栏独立滚动见
/// App.css .sidebar。
import { useCallback, useState, type MouseEvent } from "react";
import { useTranslation } from "react-i18next";

import { sortEntries, type FsEntry } from "../lib/browse";
import { zhCmp } from "../lib/collate";
import { daemonList } from "../lib/daemon";
import { sortDevices, type DeviceInfo } from "../lib/derived";
import { type Favorite } from "../lib/favorites";
import { splitPath, type Loc } from "../lib/nav";

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
  /// 行 title 提示(返工 §13.1 快捷成员行传「设备名 · 全路径」;设备树
  /// 不传,title 属性整个不挂)。
  title?: string;
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
        title={props.title}
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

/// 成员目录行标签 = 路径末段(返工 §13.1:标签=路径显示名)。路径分解走
/// lib/nav splitPath(按是否含 '\' 显式探测分隔符——nav.ts 分解纪律);全空
/// 兜底原串,如 path="/" 显示 "/" 本身。
function pathTail(path: string): string {
  const { segs } = splitPath(path);
  const tail = segs[segs.length - 1] ?? "";
  return tail.length > 0 ? tail : path;
}

/// 快捷条目节点(返工 §13.1):行本体点击=开常用视图(fav Loc);caret=
/// 展开/收起成员目录节点;右键=onFavContextMenu(T13 菜单装配);拖放落点
/// `fav\0id`(Task 12 消费)。展开键与目录节点共用一套 expandedKeys(键
/// `fav\0id` 与 `device\0path` 无碰撞),对常用行取反解释:键不在集=展开
/// (缺省展开——§13.1 组默认开,成员节点即刻可见;收藏异步到齐后新条目
/// 也天生展开),用户收起才入集。成员子节点=每成员一枚 DirNode(depth+1),
/// 与设备树节点完全同款(懒加载子目录/点击导航/落点 `device\0path`/离线
/// 灰显);成员行标签=路径末段,title=设备名 · 全路径。成员节点与其懒
/// 加载子目录节点点击同一回调 onOpenFavDir(favId, device, clickedPath)
/// ——V3-3 返工 §13.6 起进快捷命名空间(App 侧 favdirAt 归一为成员
/// 基准 base+相对段 rel,R6 修复轮),树结构不动。
function FavNode(props: {
  fav: Favorite;
  depth: number;
  /// 设备快照索引(成员在线态与 title 设备名来源)。
  deviceById: Map<string, DeviceInfo>;
  current: Loc;
  expandedKeys: Set<string>;
  cache: Map<string, DirChildren>;
  /// 拖拽悬停高亮键(device\0path / fav\0id;null = 无悬停)。
  dropHoverKey: string | null;
  onToggleFav(id: string): void;
  onOpenFav(id: string): void;
  /// 成员节点及其子目录节点点击(V3-3 返工 §13.6):进该快捷命名空间。
  onOpenFavDir(favId: string, device: string, clickedPath: string): void;
  onFavContextMenu(id: string, x: number, y: number): void;
  onToggleDir(device: string, path: string): void;
}) {
  const k = `fav\u0000${props.fav.id}`;
  const isOpen = !props.expandedKeys.has(k);
  const selected = props.current.kind === "fav" && props.current.id === props.fav.id;
  return (
    <>
      <div
        className={
          "tree-node clickable" +
          (selected ? " selected" : "") +
          (k === props.dropHoverKey ? " drop-hover" : "")
        }
        style={indent(props.depth)}
        onClick={() => props.onOpenFav(props.fav.id)}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") props.onOpenFav(props.fav.id);
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          props.onFavContextMenu(props.fav.id, e.clientX, e.clientY);
        }}
        role="button"
        tabIndex={0}
        data-drop-loc={k}
      >
        <span
          className={"caret" + (isOpen ? " open" : "")}
          aria-hidden="true"
          onClick={(e) => {
            e.stopPropagation();
            props.onToggleFav(props.fav.id);
          }}
        >
          <IcChevronRight size={12} />
        </span>
        <span className="tree-icon" aria-hidden="true">
          <IcFolder size={16} />
        </span>
        <span className="tree-label">{props.fav.name}</span>
      </div>
      {isOpen && (
        <div role="group" className="tree-children">
          {props.fav.members.map((m) => {
            const dev = props.deviceById.get(m.device);
            const devName = dev?.name ?? m.device;
            return (
              <DirNode
                key={`${m.device}\u0000${m.path}`}
                device={m.device}
                path={m.path}
                name={pathTail(m.path)}
                title={`${devName} · ${m.path}`}
                depth={props.depth + 1}
                deviceOnline={dev?.online ?? false}
                current={props.current}
                expandedKeys={props.expandedKeys}
                cache={props.cache}
                dropHoverKey={props.dropHoverKey}
                onToggle={props.onToggleDir}
                // 成员/子目录节点点击 → 快捷命名空间(返工 §13.6;DirNode
                // 组装的 dir Loc 只取 device/path,上交 App 经 favdirAt 归一)
                onOpen={(loc) => {
                  if (loc.kind === "dir") {
                    props.onOpenFavDir(props.fav.id, loc.device, loc.path);
                  }
                }}
              />
            );
          })}
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
  favorites,
  inboxPath,
  showFavorites,
  onOpenFav,
  onOpenFavDir,
  onOpenInbox,
  onFavContextMenu,
  onNavigate,
}: {
  devices: DeviceInfo[];
  selfDeviceId: string | null;
  current: Loc;
  /// 拖拽悬停高亮键(device\0path / fav\0id;null = 无悬停)。
  dropHoverKey: string | null;
  /// 快捷区(V3-3 §5.2;返工 §13.1 树形化):清单与收件箱路径;
  /// showFavorites=false(未登录)整节不渲染(含标头)。「+」新建钮已撤
  /// (返工 §13.2,R5 由左上「新建」下拉承接,favReadOnly 同批撤出)。
  favorites: Favorite[];
  inboxPath: string | null;
  showFavorites: boolean;
  onOpenFav(id: string): void;
  /// 成员节点及其子目录节点点击(V3-3 返工 §13.6):进该快捷命名空间
  /// (App 侧 favdirAt 归一为成员基准 base+相对段 rel,R6 修复轮)。
  onOpenFavDir(favId: string, device: string, clickedPath: string): void;
  onOpenInbox(): void;
  /// 常用行右键(V3-3 Task 13):目标 id + 触发点视口坐标(App 开
  /// TreeNodeMenu 装配 打开/重命名/管理成员/删除常用 四项)。
  onFavContextMenu(id: string, x: number, y: number): void;
  onNavigate(loc: Loc): void;
}) {
  const { t } = useTranslation();
  /// 快捷目录组展开态(返工 §13.1):默认展开(裁定:组开即收件箱+条目+
  /// 成员目录全可见),组节点本体点击/caret 切换。
  const [favsOpen, setFavsOpen] = useState(true);
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

  /// 快捷行 caret 切换(返工 §13.1):与目录节点共用 expandedKeys(键
  /// `fav\0id`,取反解释——见 FavNode),不涉懒加载(成员 DirNode 自管)。
  const toggleFav = useCallback((id: string) => {
    const k = `fav\u0000${id}`;
    setExpandedKeys((s) => {
      const next = new Set(s);
      if (next.has(k)) {
        next.delete(k);
      } else {
        next.add(k);
      }
      return next;
    });
  }, []);

  const sorted = sortDevices(devices, selfDeviceId);
  const gotoDevices = () => onNavigate({ kind: "devices" });
  /// 成员在线态/title 设备名索引(快捷成员行从快照 join,不入库)。
  const deviceById = new Map(devices.map((d) => [d.device_id, d]));

  // 收件箱行:拖放落点键=本机收件目录(device\0recv_dir,与目录子节点同
  // 形,现有拖放分派天然可落);selfDeviceId 缺失不挂(命中测试自然跳过)。
  // 当前导航位即收件目录时 selected 高亮。
  const inboxKey =
    showFavorites && inboxPath != null && selfDeviceId != null
      ? `${selfDeviceId}\u0000${inboxPath}`
      : null;
  const inboxSelected =
    current.kind === "dir" &&
    selfDeviceId != null &&
    current.device === selfDeviceId &&
    current.path === inboxPath;

  return (
    <div className="sidebar">
      {showFavorites && (
        <>
          {/* 快捷区组节点(返工 §13.1):与「Synshu 设备」组同款 caret,
              默认展开;行本体点击/caret=切换展开(组无导航 Loc)。右端
              「+」新建钮已撤(返工 §13.2,R5 由左上「新建」下拉承接) */}
          <div
            className="tree-node clickable"
            onClick={() => setFavsOpen((o) => !o)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") setFavsOpen((o) => !o);
            }}
            role="button"
            tabIndex={0}
          >
            <span
              className={"caret" + (favsOpen ? " open" : "")}
              aria-hidden="true"
              onClick={(e) => {
                e.stopPropagation();
                setFavsOpen((o) => !o);
              }}
            >
              <IcChevronRight size={12} />
            </span>
            <span className="tree-label strong">{t("sidebar.favorites")}</span>
          </div>
          {favsOpen && (
            <>
              {/* 置顶收件箱行(V3-3 §5.2,组内 depth 1):文件夹图标+
                  「收件箱」;点击进本机收件目录,拖放落点见上 inboxKey */}
              {inboxPath != null && (
                <div
                  className={
                    "tree-node clickable" +
                    (inboxSelected ? " selected" : "") +
                    (inboxKey != null && inboxKey === dropHoverKey ? " drop-hover" : "")
                  }
                  style={indent(1)}
                  onClick={onOpenInbox}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") onOpenInbox();
                  }}
                  role="button"
                  tabIndex={0}
                  data-drop-loc={inboxKey ?? undefined}
                >
                  <span className="tree-icon" aria-hidden="true">
                    <IcFolder size={16} />
                  </span>
                  <span className="tree-label">{t("sidebar.inbox")}</span>
                </div>
              )}
              {/* 快捷条目行(FavNode,组内 depth 1,成员目录 depth 2):
                  点击=开常用视图,caret=展开成员目录节点,右键=管理菜单,
                  拖放落点 fav\0id(Task 12 消费) */}
              {favorites.map((f) => (
                <FavNode
                  key={f.id}
                  fav={f}
                  depth={1}
                  deviceById={deviceById}
                  current={current}
                  expandedKeys={expandedKeys}
                  cache={cache}
                  dropHoverKey={dropHoverKey}
                  onToggleFav={toggleFav}
                  onOpenFav={onOpenFav}
                  onOpenFavDir={onOpenFavDir}
                  onFavContextMenu={onFavContextMenu}
                  onToggleDir={toggleNode}
                />
              ))}
            </>
          )}
        </>
      )}
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
