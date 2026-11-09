/// 左侧树:「快捷目录」区(V3-3 §5.2 点亮;返工波 4 §16.3 树=成员节点)+
/// 「Synshu 设备」可展开节点。
///
/// 快捷区(返工波 4 §16.3,2026-11-07 钦定;§14.1 内容并集废止,§13.1
/// 形态回归;返工裁定五 §17.1/§17.2 改组头与收件箱行;§17.5 树子级含
/// 文件):「快捷目录」组
/// 行=与「Synshu 设备」组行同构——行本体点击=导航落快捷目录着陆位
/// {kind:"favs"}(§17.1 右侧渲染着陆页;§15.2 落收件箱裁定就此废止;
/// 当前位=着陆位时行 selected 高亮),caret=展开/收起(键盘 Enter/Space
/// 与行点击同义=导航)。组内=置顶收件箱行 → 快捷条目行(FavNode):
/// 收件箱行自 §17.2 起与成员节点同构——DirNode 原样复用(device=self、
/// path=recv_dir):行本体点击=进本机收件目录浏览,caret=懒加载展开
/// 收件目录子级(子级含文件行,§17.5 两端对齐),selected 高亮/拖放
/// 落点 `device\0recv_dir` 与目录节点同一套机制;快捷条目行(FavNode):
/// 行本体点击=开快捷视图(fav Loc);行 caret=展开/收起成员目录子树——
/// 每成员一枚 DirNode(depth+1),标签=「设备名 - 目录名」(§16.3 词面,
/// 与主区分组节头同款;设备不在快照回退设备 id),完全复刻设备树语义:
/// 懒加载子目录(daemonList(member.device, member.path) 起,子级逐级)、
/// 点击导航 onOpenFavDir(favId, device, path)(App 侧 favdirAt 归一为
/// base=成员路径+rel=相对段,任何深度同规范形态)、拖放落点
/// `device\0path`、离线成员连同其子树灰显禁点禁展开(在线态从 devices
/// 快照 join,快照缺该设备按离线)。离线成员不再注记行——成员节点自身
/// 灰显即披露(§14.1 尾注记行「N 个成员离线」随内容并集一并废止);零
/// 成员快捷展开仍给引导占位行(favorites.emptyHint,R4 披露③保留)。
/// 右键=onFavContextMenu(id, x, y)(T13:App 据此开 TreeNodeMenu——打开/
/// 重命名/管理成员/删除快捷)保留;右端「+」新建钮已撤(§13.2,R5 由左上
/// 「新建」下拉承接)。展开键共用一套 expandedKeys:目录键 `device\0path`,
/// 快捷行键 `fav\0id`(无碰撞;快捷行取反解释——缺省展开,用户收起才入
/// 集)。拖放落点(Task 12 消费):收件箱行 `device\0recv_dir`、快捷行
/// `fav\0id` 保持不变;快捷树成员行落点 `device\0path` 由 DirNode 自挂。
/// 色点零消费(§16.2 色标全量移除,色板库休眠保留)。showFavorites=false
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
/// 整体灰显,点略);目录子节点与快捷成员节点=16px 文件夹图标。分组
/// 标头不带文件图标(裁定记档);侧栏独立滚动见 App.css .sidebar。
import { useCallback, useState, type MouseEvent } from "react";
import { useTranslation } from "react-i18next";

import { sortEntries, type FsEntry } from "../lib/browse";
import { zhCmp } from "../lib/collate";
import { daemonList } from "../lib/daemon";
import { sortDevices, type DeviceInfo } from "../lib/derived";
import { type Favorite } from "../lib/favorites";
import { splitPath, type Loc } from "../lib/nav";

import { IcChevronRight, IcFile, IcFolder, IcPc, IcPhone } from "./icons";

/// 目录子节点缓存条目(懒加载结果;failed 时下次展开重试)。entries=该
/// 目录**全部**条目(目录+文件,目录恒前+名称升序):渲染层按面滤形——
/// 设备树滤回仅目录(§17.5 设备树维持仅目录),快捷树面(成员节点/
/// 收件箱行/其子级)子级含文件行;缓存整单保留维持既有加载管线不动。
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

/// 平台图标(设备根节点,与 DeviceTable/AddressBar 同裁定):android/ios
/// → 手机形(§18.3:ios 曾落 PC 形=bug);windows/linux/macos 及未知/
/// 缺失 → PC 形兜底。
function PlatformIcon({ platform }: { platform: string | undefined }) {
  return platform === "android" || platform === "ios" ? (
    <IcPhone size={16} />
  ) : (
    <IcPc size={16} />
  );
}

/// 成员行目录名 = 路径末段(§16.3「设备名 - 目录名」的后半,主区分组
/// 节头同词面)。路径分解经 lib/nav splitPath(分隔符纪律:按是否含 '\'
/// 显式探测,不得另写解析);全空兜底原串,如 path="/" 显示 "/" 本身。
function pathTail(path: string): string {
  const { segs } = splitPath(path);
  const tail = segs[segs.length - 1] ?? "";
  return tail.length > 0 ? tail : path;
}

/// 文件行(§17.5 快捷树子级含文件,返工二 §14.1 同形复辟;色点已随
/// §16.2 移除不复活):叶,无 caret 交互、无点击/键盘/落点/选中/拖拽
/// (仅展示,树形对齐留 caret 占位槽);字色弱化走 CSS .tree-file。
/// 设备树不渲染(showFiles 不传,零回归)。
function FileRow(props: { name: string; depth: number }) {
  return (
    <div className="tree-node tree-file" style={indent(props.depth)}>
      <span className="caret" aria-hidden="true" />
      <span className="tree-icon" aria-hidden="true">
        <IcFile size={16} />
      </span>
      <span className="tree-label">{props.name}</span>
    </div>
  );
}

/// 树节点(设备节点与目录子节点共用;快捷树成员节点同形,§16.3):caret
/// 切换展开,label 点击导航。全部状态(展开集/缓存)由 Sidebar 持有,本
/// 组件无内部状态,可安全递归。禁用态由 deviceOnline 派生并沿递归传递:
/// 离线设备的根与其已展开的目录子节点(含快捷的离线成员节点)一并灰显
/// 禁点禁展开(与主区 canEnterDir 门控同源)。显示名经 name 传入——设备
/// 树=目录名,快捷成员节点=「设备名 - 目录名」(§16.3 词面)。
/// §17.5 fav 形态开关 showFiles:子级含文件行(叶,FileRow 仅展示)且沿
/// 子目录递归传递(快捷树面=成员/收件箱/子目录全域);设备树不传=false,
/// 子级只显示目录(零回归)。
/// DirNode 供本文件 Sidebar 组装;具名导出仅供 SSR 冒烟直驱钉结构
/// (展开态/缓存显式注入,渲染期零拉取——沿 FavTreeRows 先例)。
export function DirNode(props: {
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
  /// §17.5 fav 形态开关:子级含文件行(设备树不传=false,子级只显示
  /// 目录)。
  showFiles?: boolean;
  /// 显示隐藏项(V3-4 §5-3;缺省 false = 滤 dotfile——目录+文件同滤,
  /// 与主区 rows 管线同一开关面):true = 全量透出。
  showHidden?: boolean;
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
  // 显示隐藏项(V3-4 §5-3):先滤 dotfile 再按面滤形(过滤基元 = 名称
  // 以 "." 开头,与主区 filterHiddenEntries 同口径)。
  const visible = (props.showHidden ?? false)
    ? (child?.entries ?? [])
    : (child?.entries ?? []).filter((e) => !e.name.startsWith("."));
  // 子级切片:目录组恒渲染;文件组仅 fav 形态(§17.5 快捷树面)。缓存
  // 缺失(未加载)时两组恒空——设备树空态判定沿「无目录子级」(只含
  // 文件的目录在设备树仍视为空,零回归),fav 形态判「无任何条目」。
  const dirs = visible.filter((e) => e.is_dir);
  const files = props.showFiles ? visible.filter((e) => !e.is_dir) : [];
  const isEmpty =
    child != null &&
    !child.loading &&
    !child.failed &&
    (props.showFiles ? visible.length === 0 : dirs.length === 0);
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
              showHidden={props.showHidden}
              onToggle={props.onToggle}
              onOpen={props.onOpen}
            />
          ))}
          {files.map((f) => (
            <FileRow key={f.path} name={f.name} depth={props.depth + 1} />
          ))}
        </div>
      )}
    </>
  );
}

/// 快捷条目节点(返工波 4 §16.3):行本体点击=开快捷视图(fav Loc);
/// caret=展开/收起成员目录子树(每成员一枚 DirNode);右键=onFavContextMenu
/// (T13 菜单装配);拖放落点 `fav\0id`(Task 12 消费)。展开键与目录节点
/// 共用一套 expandedKeys(键 `fav\0id` 与 `device\0path` 无碰撞),对快捷
/// 行取反解释:键不在集=展开(缺省展开——组默认开,成员节点即刻可见),
/// 用户收起才入集。成员子节点=每成员一枚 DirNode(depth+1),与设备树
/// 节点同款(懒加载/点击导航/落点 `device\0path`/离线灰显)而子级含
/// 文件行(§17.5 showFiles 沿子目录递归全域);
/// 标签=「设备名 - 目录名」(§16.3,设备不在快照回退设备 id);成员行与
/// 其懒加载子目录行点击同走 onOpenFavDir(App 侧 favdirAt 归一)。
function FavNode(props: {
  fav: Favorite;
  depth: number;
  /// 设备快照索引(成员在线态与设备名来源;快照缺该设备按离线/回退 id)。
  deviceById: Map<string, DeviceInfo>;
  current: Loc;
  expandedKeys: Set<string>;
  cache: Map<string, DirChildren>;
  /// 拖拽悬停高亮键(device\0path / fav\0id;null = 无悬停)。
  dropHoverKey: string | null;
  /// 显示隐藏项(V3-4 §5-3):透传成员 DirNode 子级。
  showHidden: boolean;
  onToggleFav(id: string): void;
  onOpenFav(id: string): void;
  /// 成员节点与其懒加载子目录行点击(V3-3 返工 §13.6 沿袭):进该快捷
  /// 命名空间(App 侧 favdirAt 归一)。
  onOpenFavDir(favId: string, device: string, clickedPath: string): void;
  onFavContextMenu(id: string, x: number, y: number): void;
  onToggleDir(device: string, path: string): void;
}) {
  const { t } = useTranslation();
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
            const devName = props.deviceById.get(m.device)?.name ?? m.device;
            return (
              <DirNode
                key={`${m.device}\u0000${m.path}`}
                device={m.device}
                path={m.path}
                name={`${devName} - ${pathTail(m.path)}`}
                depth={props.depth + 1}
                deviceOnline={props.deviceById.get(m.device)?.online ?? false}
                current={props.current}
                expandedKeys={props.expandedKeys}
                cache={props.cache}
                dropHoverKey={props.dropHoverKey}
                // §17.5:快捷树子级含文件(成员节点子级+子目录递归全域)
                showFiles={true}
                showHidden={props.showHidden}
                onToggle={props.onToggleDir}
                // 成员节点/子目录行组装的 dir Loc 只取 device/path,上交
                // App 经 favdirAt 归一为快捷命名空间位(§13.6 沿袭)
                onOpen={(loc) => {
                  if (loc.kind === "dir") {
                    props.onOpenFavDir(props.fav.id, loc.device, loc.path);
                  }
                }}
              />
            );
          })}
          {props.fav.members.length === 0 && (
            /* 零成员展开占位(R4 披露③,R8 保留):空快捷目录展开不再是
               空百叶——引导行指向设备浏览添加入口;文案复用
               favorites.emptyHint,样式 tree-status 同空目录形 */
            <div className="tree-status" style={indent(props.depth + 1)}>
              {t("favorites.emptyHint")}
            </div>
          )}
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
  showHidden = false,
  onOpenFav,
  onOpenFavDir,
  onFavContextMenu,
  onNavigate,
}: {
  devices: DeviceInfo[];
  selfDeviceId: string | null;
  current: Loc;
  /// 拖拽悬停高亮键(device\0path / fav\0id;null = 无悬停)。
  dropHoverKey: string | null;
  /// 快捷区(V3-3 §5.2;返工波 4 §16.3 树=成员节点):清单与收件箱路径;
  /// showFavorites=false(未登录)整节不渲染(含标头)。「+」新建钮已撤
  /// (返工 §13.2,R5 由左上「新建」下拉承接)。
  favorites: Favorite[];
  inboxPath: string | null;
  showFavorites: boolean;
  /// 显示隐藏项(V3-4 §5-3,缺省 false = 滤 dotfile,零改动调用面语义
  /// 同旧行为):false 时树子级滤除 dotfile(目录+文件同滤)。
  showHidden?: boolean;
  onOpenFav(id: string): void;
  /// 成员节点及其子目录行点击(V3-3 返工 §13.6):进该快捷命名空间
  /// (App 侧 favdirAt 归一为成员基准 base+相对段 rel,R6 修复轮)。
  onOpenFavDir(favId: string, device: string, clickedPath: string): void;
  /// 快捷行右键(V3-3 Task 13):目标 id + 触发点视口坐标(App 开
  /// TreeNodeMenu 装配 打开/重命名/管理成员/删除快捷 四项)。
  onFavContextMenu(id: string, x: number, y: number): void;
  onNavigate(loc: Loc): void;
}) {
  const { t } = useTranslation();
  /// 快捷目录组展开态(§15.2 组头行 caret 专属;行本体点击=导航落收件
  /// 箱):默认展开(组开即收件箱+条目+成员节点全可见)。
  const [favsOpen, setFavsOpen] = useState(true);
  const [devicesOpen, setDevicesOpen] = useState(false);
  const [expandedKeys, setExpandedKeys] = useState<Set<string>>(new Set());
  const [cache, setCache] = useState<Map<string, DirChildren>>(new Map());

  /// 懒加载某设备某目录子级(缓存整单条目,目录+文件,目录恒前+名称
  /// 升序;渲染层按面滤形——设备树滤回仅目录,快捷树面子级含文件行,
  /// §17.5)。
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
  /// `fav\0id`,取反解释——见 FavNode),不涉懒加载(成员节点自管)。
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
  /// 成员节点在线态/设备名索引(成员子树从快照 join,不入库)。
  const deviceById = new Map(devices.map((d) => [d.device_id, d]));

  // 收件箱行(返工裁定五 §17.2)= DirNode 原样复用:拖放落点键
  // (device\0recv_dir)、selected 高亮、caret 懒加载展开全部由 DirNode
  // 同一套机制自管,无需 Sidebar 另算键/高亮(self 设备恒本地在线,
  // deviceOnline 恒 true;selfDeviceId/recv_dir 缺席整行不渲染)。

  return (
    <div className="sidebar">
      {showFavorites && (
        <>
          {/* 快捷区组行(返工裁定五 §17.1,推翻 §15.2 落收件箱):与
              「Synshu 设备」组行同构——行本体点击/键盘=导航落快捷目录
              着陆位(App 右侧渲染着陆页),caret=切换展开(stopPropagation
              防串导航);当前位=着陆位时 selected 高亮。右端「+」新建钮
              已撤(返工 §13.2,R5 由左上「新建」下拉承接) */}
          <div
            className={
              "tree-node clickable" + (current.kind === "favs" ? " selected" : "")
            }
            onClick={() => onNavigate({ kind: "favs" })}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") onNavigate({ kind: "favs" });
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
              {/* 置顶收件箱行(§17.2 成员语义对齐):DirNode 原样复用
                  (device=self,path=recv_dir,depth 1)——行点击进本机收件
                  目录,caret 懒加载展开收件目录子级(子级含文件行,§17.5
                  两端对齐),selected/拖放落点与目录节点同机制 */}
              {inboxPath != null && selfDeviceId != null && (
                <DirNode
                  device={selfDeviceId}
                  path={inboxPath}
                  name={t("sidebar.inbox")}
                  depth={1}
                  deviceOnline={true}
                  showFiles={true}
                  showHidden={showHidden}
                  current={current}
                  expandedKeys={expandedKeys}
                  cache={cache}
                  dropHoverKey={dropHoverKey}
                  onToggle={toggleNode}
                  // 行点击(本行与懒加载子级行同一入口):导航到所点目录
                  // (本行=收件目录本身,与 App openInbox 同落点;子级=逐级
                  // 下钻)。onNavigate 零回归承担。
                  onOpen={(loc) => {
                    if (loc.kind === "dir") onNavigate(loc);
                  }}
                />
              )}
              {/* 快捷条目行(FavNode,组内 depth 1,成员节点 depth 2):
                  点击=开快捷视图,caret=展开成员目录子树(§16.3),
                  右键=管理菜单,拖放落点 fav\0id(Task 12 消费) */}
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
                  showHidden={showHidden}
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
              showHidden={showHidden}
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
