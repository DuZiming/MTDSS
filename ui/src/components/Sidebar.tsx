/// 左侧树:「快捷目录」区(V3-3 §5.2 点亮;返工二 §14.1 树形=成员内容合并)+
/// 「Synshu 设备」可展开节点。
///
/// 快捷区(返工二 §14.1,2026-11-06 钦定,推翻 §13.1 成员目录节点形态):
/// 「快捷目录」组节点=与「Synshu 设备」组同款可展开 caret,默认展开 →
/// 置顶收件箱行(本机收件目录快捷位)→ 快捷条目行(FavNode):行本体点击
/// =开快捷视图(fav Loc,混合视图);行 caret=展开/收起成员**内容合并**子树
/// ——子级=各**在线**成员目录列表的并集(文件与文件夹混排、目录恒前 +
/// 名称升序,lib/favtree mergeMemberListings;lib/favfetch useFavListing
/// 渐进扇出,应答逐成员到达逐次并入),每行带来源终端色点(colors 取
/// deviceColorMap)。目录行=DirNode fav 形态(色点 + 子级含文件行):
/// caret 懒加载**该目录在其所属设备上的子级**(daemonList(device, path)
/// 起,子级逐级;同一 Sidebar cache/expandedKeys 机制,键 `device\0path`),
/// 点击导航 onOpenFavDir(favId, device, clickedPath)——V3-3 返工 §13.6
/// 起进快捷命名空间(App 侧经 favdirAt 归一为 base=成员路径+rel=相对段,
/// 任何深度同规范形态);文件行=仅展示不响应(叶,无 caret 交互、无落点、
/// 无 hover/选中——内容树形态,与设备树「子级只显示目录」分立)。离线成员
/// 条目缺席;≥1 名离线/失败时子级尾部注记行「N 个成员离线」(tree-status);
/// 加载中给 spinner 行,行渐进出现不等齐;零成员快捷展开仍给引导占位行
/// (favorites.emptyHint,样式 tree-status)。快捷树文件行各级可见(内容
/// 树);**设备树不动**(子级仍只显示目录,零回归)。
///
/// 右键=onFavContextMenu(id, x, y)(T13:App 据此开 TreeNodeMenu——打开/
/// 重命名/管理成员/删除快捷)保留;右端「+」新建钮已撤(§13.2,R5 由左上
/// 「新建」下拉承接)。展开键共用一套 expandedKeys:目录键 `device\0path`,
/// 快捷行键 `fav\0id`(无碰撞;快捷行取反解释——缺省展开,用户收起才入集)。
/// 拖放落点(Task 12 消费):收件箱行 `device\0recv_dir`、快捷行 `fav\0id`
/// 保持不变;快捷树目录行落点 `device\0path` 由 DirNode 自挂(文件行不挂)。
/// showFavorites=false(未登录)整节不渲染(含标头),未登录面与 V2 一致
/// =零回归。
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
/// 整体灰显,点略);目录子节点=16px 文件夹图标;文件行=16px 文件图标
/// +弱化字色。「Synshu 设备」「快捷目录」两根行是分组标头,不带文件图标
/// (裁定记档);侧栏独立滚动见 App.css .sidebar。
import { useCallback, useState, type MouseEvent } from "react";
import { useTranslation } from "react-i18next";

import { sortEntries, type FsEntry } from "../lib/browse";
import { zhCmp } from "../lib/collate";
import { daemonList } from "../lib/daemon";
import { sortDevices, type DeviceInfo } from "../lib/derived";
import { useFavListing } from "../lib/favfetch";
import { absentMemberCount, mergeMemberListings } from "../lib/favtree";
import { type Favorite } from "../lib/favorites";
import { type MixedRow } from "../lib/mixedview";
import { type Loc } from "../lib/nav";

import { IcChevronRight, IcFile, IcFolder, IcPc, IcPhone } from "./icons";

/// 目录子节点缓存条目(懒加载结果;failed 时下次展开重试)。entries=该
/// 目录**全部**条目(目录+文件,目录恒前+名称升序):设备树渲染层滤回
/// 仅目录(零回归),快捷树 fav 形态整单可见(§14.1 内容树)。
interface DirChildren {
  loading: boolean;
  entries: FsEntry[];
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

/// 来源终端色点(§14.1 合并行/§4 色板):colors.get(设备 id),混合视图
/// 行点同款 .src-dot。
function SourceDot({ color }: { color: string | undefined }) {
  if (color == null) return null;
  return <span className="src-dot" style={{ background: color }} aria-hidden="true" />;
}

/// 文件行(§14.1 内容树,快捷树各级可见):叶,无 caret 交互、无点击/
/// 键盘/落点/选中(仅展示,树形对齐留 caret 占位槽);字色弱化走 CSS
/// .tree-file。设备树不渲染(零回归)。
function FileRow(props: { name: string; depth: number; dotColor: string | undefined }) {
  return (
    <div className="tree-node tree-file" style={indent(props.depth)}>
      <span className="caret" aria-hidden="true" />
      <span className="tree-icon" aria-hidden="true">
        <IcFile size={16} />
      </span>
      <SourceDot color={props.dotColor} />
      <span className="tree-label">{props.name}</span>
    </div>
  );
}

/// 树节点(设备节点与目录子节点共用):caret 切换展开,label 点击导航。
/// 全部状态(展开集/缓存)由 Sidebar 持有,本组件无内部状态,可安全递归。
/// 禁用态由 deviceOnline 派生并沿递归传递:离线设备的根与其已展开的目录
/// 子节点一并灰显禁点禁展开(与主区 canEnterDir 门控同源)。
/// §14.1 fav 形态(设备树不传=零回归):showFiles=子级含文件行(仅展示);
/// dotColor=来源终端色点(§4 色板,合并行与懒加载子级沿设备传递)。
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
  /// 拖拽悬停高亮键(device\0path / fav\0id;null = 无悬停)。
  dropHoverKey: string | null;
  /// §14.1 fav 形态开关:子级含文件行(设备树不传=false,子级只显示目录)。
  showFiles?: boolean;
  /// 来源终端色点(快捷树传,设备树不传=不渲染)。
  dotColor?: string;
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
  // 子级切片:目录组恒渲染;文件组仅 fav 形态(§14.1 内容树)。缓存缺失
  // (未加载)时两组恒空——设备树空态判定沿「无目录子级」(只含文件的
  // 目录在设备树仍视为空,零回归),fav 形态判「无任何条目」。
  const dirs = child?.entries.filter((e) => e.is_dir) ?? [];
  const files = props.showFiles ? (child?.entries.filter((e) => !e.is_dir) ?? []) : [];
  const isEmpty =
    child != null && !child.loading && !child.failed && (props.showFiles ? child.entries.length === 0 : dirs.length === 0);
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
        <SourceDot color={props.dotColor} />
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
          {isEmpty && (
            <div className="tree-status" style={indent(props.depth + 1)}>
              {t("sidebar.emptyDir")}
            </div>
          )}
          {dirs.map((d) => (
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
              showFiles={props.showFiles}
              dotColor={props.dotColor}
              onToggle={props.onToggle}
              onOpen={props.onOpen}
            />
          ))}
          {files.map((f) => (
            <FileRow key={f.path} name={f.name} depth={props.depth + 1} dotColor={props.dotColor} />
          ))}
        </div>
      )}
    </>
  );
}

/// 快捷树合并子级(§14.1 呈现层,纯展示无引擎——SSR 冒烟直驱钉结构;
/// 引擎在 FavChildren):目录行=DirNode fav 形态(色点+子级含文件行,点击
/// 上交 onOpenFavDir),文件行=FileRow(仅展示);加载中行+尾部「N 个成员
/// 离线」注记行(absent>0 且加载已定,行渐进出现不打断)+零成员引导行。
export function FavTreeRows(props: {
  favId: string;
  rows: MixedRow[];
  loading: boolean;
  absent: number;
  /// 零成员快捷(0 成员):引导占位行(favorites.emptyHint,R4 披露③)。
  empty: boolean;
  depth: number;
  colors: Map<string, string>;
  /// 设备快照索引(合并目录行在线态来源;快照缺该设备按离线)。
  deviceById: Map<string, DeviceInfo>;
  current: Loc;
  expandedKeys: Set<string>;
  cache: Map<string, DirChildren>;
  dropHoverKey: string | null;
  onToggleDir(device: string, path: string): void;
  /// 目录行与其懒加载子目录行点击(V3-3 返工 §13.6):进该快捷命名空间。
  onOpenFavDir(favId: string, device: string, clickedPath: string): void;
}) {
  const { t } = useTranslation();
  return (
    <div role="group" className="tree-children">
      {props.loading && (
        <div className="tree-status loading" style={indent(props.depth)}>
          <span className="spinner" aria-hidden="true" />
          {t("sidebar.loading")}
        </div>
      )}
      {props.rows.map((r) =>
        r.entry.is_dir ? (
          <DirNode
            key={r.key}
            device={r.device}
            path={r.entry.path}
            name={r.entry.name}
            depth={props.depth}
            deviceOnline={props.deviceById.get(r.device)?.online ?? false}
            current={props.current}
            expandedKeys={props.expandedKeys}
            cache={props.cache}
            dropHoverKey={props.dropHoverKey}
            showFiles
            dotColor={props.colors.get(r.device)}
            onToggle={props.onToggleDir}
            // 合并行/子目录行点击 → 快捷命名空间(返工 §13.6;DirNode 组装
            // 的 dir Loc 只取 device/path,上交 App 经 favdirAt 归一)
            onOpen={(loc) => {
              if (loc.kind === "dir") {
                props.onOpenFavDir(props.favId, loc.device, loc.path);
              }
            }}
          />
        ) : (
          <FileRow key={r.key} name={r.entry.name} depth={props.depth} dotColor={props.colors.get(r.device)} />
        ),
      )}
      {!props.loading && props.absent > 0 && (
        /* 不可用成员注记(§14.1):离线/失败成员条目缺席,尾部注记「N 个
           成员离线」(failed 并入同注记,口径=mixedview unavailableCount) */
        <div className="tree-status" style={indent(props.depth)}>
          {t("sidebar.favOffline", { n: props.absent })}
        </div>
      )}
      {props.empty && (
        /* 零成员展开占位(R4 披露③,R8):空快捷目录展开不再是空百叶
           ——引导行指向设备浏览添加入口;文案复用 favorites.emptyHint,
           与混合视图空态同词面;样式 tree-status 同空目录形 */
        <div className="tree-status" style={indent(props.depth)}>
          {t("favorites.emptyHint")}
        </div>
      )}
    </div>
  );
}

/// 展开的快捷子树引擎座(§14.1):lib/favfetch useFavListing 每展开实例一
/// 个(与 FavView 同引擎同纪律,各挂各的实例互不串台)。reloadTick 恒 0
/// ——子树随展开挂载/收起卸载:**重开即新世代**(重拉在线成员),挂载期
/// devices 快照变化走引擎软更新(未定成员 pending↔offline 翻桶,已定不
/// 重拉);离线成员种子即 offline 桶从不发请求。同 id 快捷 members 变更
/// 不重拉(引擎已知边界,随收起/展开收口,与 FavView 一致)。
function FavChildren(props: {
  fav: Favorite;
  depth: number;
  devices: DeviceInfo[];
  colors: Map<string, string>;
  deviceById: Map<string, DeviceInfo>;
  current: Loc;
  expandedKeys: Set<string>;
  cache: Map<string, DirChildren>;
  dropHoverKey: string | null;
  onToggleDir(device: string, path: string): void;
  onOpenFavDir(favId: string, device: string, clickedPath: string): void;
}) {
  const snap = useFavListing(props.fav, props.devices, 0);
  const rows = mergeMemberListings(snap.states, zhCmp());
  return (
    <FavTreeRows
      favId={props.fav.id}
      rows={rows}
      loading={snap.loading}
      absent={absentMemberCount(snap.states)}
      empty={props.fav.members.length === 0}
      depth={props.depth}
      colors={props.colors}
      deviceById={props.deviceById}
      current={props.current}
      expandedKeys={props.expandedKeys}
      cache={props.cache}
      dropHoverKey={props.dropHoverKey}
      onToggleDir={props.onToggleDir}
      onOpenFavDir={props.onOpenFavDir}
    />
  );
}

/// 快捷条目节点(返工二 §14.1):行本体点击=开快捷视图(fav Loc);caret=
/// 展开/收起成员内容合并子树(FavChildren);右键=onFavContextMenu(T13
/// 菜单装配);拖放落点 `fav\0id`(Task 12 消费)。展开键与目录节点共用
/// 一套 expandedKeys(键 `fav\0id` 与 `device\0path` 无碰撞),对快捷行
/// 取反解释:键不在集=展开(缺省展开——§13.1 组默认开,合并子级即刻渐进
/// 可见),用户收起才入集。
function FavNode(props: {
  fav: Favorite;
  depth: number;
  /// 设备快照(引擎在线分桶)与快照索引(合并行在线态来源)。
  devices: DeviceInfo[];
  deviceById: Map<string, DeviceInfo>;
  /// 来源终端色板(§4 deviceColorMap,App 下发)。
  colors: Map<string, string>;
  current: Loc;
  expandedKeys: Set<string>;
  cache: Map<string, DirChildren>;
  /// 拖拽悬停高亮键(device\0path / fav\0id;null = 无悬停)。
  dropHoverKey: string | null;
  onToggleFav(id: string): void;
  onOpenFav(id: string): void;
  /// 目录行与其懒加载子目录行点击(V3-3 返工 §13.6):进该快捷命名空间。
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
        <FavChildren
          fav={props.fav}
          depth={props.depth + 1}
          devices={props.devices}
          colors={props.colors}
          deviceById={props.deviceById}
          current={props.current}
          expandedKeys={props.expandedKeys}
          cache={props.cache}
          dropHoverKey={props.dropHoverKey}
          onToggleDir={props.onToggleDir}
          onOpenFavDir={props.onOpenFavDir}
        />
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
  colors,
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
  /// 快捷区(V3-3 §5.2;返工二 §14.1 树形=成员内容合并):清单与收件箱
  /// 路径;showFavorites=false(未登录)整节不渲染(含标头)。「+」新建钮
  /// 已撤(返工 §13.2,R5 由左上「新建」下拉承接)。
  favorites: Favorite[];
  inboxPath: string | null;
  showFavorites: boolean;
  /// 来源终端色板(§4 deviceColorMap;App 下发,合并行色点取色)。
  colors: Map<string, string>;
  onOpenFav(id: string): void;
  /// 目录行及其子目录行点击(V3-3 返工 §13.6):进该快捷命名空间
  /// (App 侧 favdirAt 归一为成员基准 base+相对段 rel,R6 修复轮)。
  onOpenFavDir(favId: string, device: string, clickedPath: string): void;
  onOpenInbox(): void;
  /// 快捷行右键(V3-3 Task 13):目标 id + 触发点视口坐标(App 开
  /// TreeNodeMenu 装配 打开/重命名/管理成员/删除快捷 四项)。
  onFavContextMenu(id: string, x: number, y: number): void;
  onNavigate(loc: Loc): void;
}) {
  const { t } = useTranslation();
  /// 快捷目录组展开态(返工 §13.1):默认展开(裁定:组开即收件箱+条目+
  /// 合并子级全可见),组节点本体点击/caret 切换。
  const [favsOpen, setFavsOpen] = useState(true);
  const [devicesOpen, setDevicesOpen] = useState(false);
  const [expandedKeys, setExpandedKeys] = useState<Set<string>>(new Set());
  const [cache, setCache] = useState<Map<string, DirChildren>>(new Map());

  /// 懒加载某设备某目录子级(缓存整单条目,目录+文件,目录恒前+名称升序;
  /// 设备树渲染层滤回仅目录,快捷树 fav 形态整单可见)。
  const loadChildren = useCallback((device: string, path: string) => {
    const k = dirKey(device, path);
    setCache((m) => new Map(m).set(k, { loading: true, entries: [], failed: false }));
    daemonList(device, path)
      .then((l) => {
        const entries = sortEntries(l.entries, { key: "name", dir: "asc" }, zhCmp());
        setCache((m) => new Map(m).set(k, { loading: false, entries, failed: false }));
      })
      .catch(() => {
        setCache((m) => new Map(m).set(k, { loading: false, entries: [], failed: true }));
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
  /// `fav\0id`,取反解释——见 FavNode),不涉懒加载(合并子级自管)。
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
  /// 合并行在线态/title 设备名索引(快捷子树从快照 join,不入库)。
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
              {/* 快捷条目行(FavNode,组内 depth 1,合并子级 depth 2):
                  点击=开快捷视图,caret=展开成员内容合并子树(§14.1),
                  右键=管理菜单,拖放落点 fav\0id(Task 12 消费) */}
              {favorites.map((f) => (
                <FavNode
                  key={f.id}
                  fav={f}
                  depth={1}
                  devices={devices}
                  deviceById={deviceById}
                  colors={colors}
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
