/// 地址栏(P4b Task 4 重设计,spec §7):←/→/↑ SVG 导航按钮(禁用灰)+
/// **设备 chip**(浏览期间恒定:平台图标+设备名+chevron,devbg 药丸;
/// 设备层呈「设备列表」灰态;点击弹全部设备下拉——在线可选、离线灰显
/// 禁选、当前项勾选,选中=进入该设备根并入导航历史)+ 面包屑(trail =
/// 根段 ▸ 第二级 chip/段 ▸ 路径段:设备位根段复用 sidebar.devices 键
/// (「Synshu 设备」)可点回设备层——设备层非传输落点,不挂
/// data-drop-loc;设备层与 favs 着陆位根段即当前位粗体不可点;设备名不
/// 设独立 crumb,由设备 chip 承担;路径段沿用现有逻辑:pathSegments 数
/// 据源、locAtSegment 点击导航、data-drop-loc 拖放落点、末段粗体不可点)+
/// 复制路径(纯图标;设备层与
/// 卷列表位 path="" 均禁用,写入经 App 的 onCopyPath)+ 搜索框(行为不变)。
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
///
/// 快捷命名空间位(V3-3 返工 §13.6;返工二 §14.3 命名空间修正;返工裁定
/// 六 §18.1 chip 统一):favdir 面包屑 = `快捷目录`(根 crumb,§18.1 起
/// 点击回 favs 着陆位——曾回该快捷混合视图)> 快捷名 chip(与设备 chip
/// 同一枚渲染器/devchip 同款,下拉恒含收件箱置顶+全部快捷目录、当前项勾
/// 选;设备 chip 在此位隐藏——设备名与原始路径不入面包屑)> rel 段(命名
/// 空间内点击导航,末段粗体不可点)。编辑态 §15.5 起启用:预填快捷命名空
/// 间虚拟地址(`快捷目录\{名}\{rel}`,lib/address favdirAddressDraft 生产
/// 侧,回车经 parseAddressInput 快捷语法原位往返),不再预填原生路径;复
/// 制路径可用(App 给成员原始绝对路径);搜索框照常(App 同一 filterEntries
/// 管线)。fav 位(混合视图)§14.3 起同入命名空间:根段「快捷目录」(§18.1
/// 起点击回着陆位,曾回设备层)+ 快捷名 chip 带下拉;设备 chip 仍隐藏,编
/// 辑态仍禁入(无单一原生路径,§15.5);搜索框 §15.5 起启用(逐节滤行,
/// FavView 既有管线)。收件箱 dir 位(本机 recv_dir;App 经 favCrumbs.
/// inboxBase 下发比较基准、selfDeviceId 门在 App 侧)§14.3 同入命名空间,
/// §18.1 起收件箱不再是特殊无下拉裸段:收件箱=chip 的合法当前值(可下拉
/// 切换),recv_dir 之下余段照常在其下;设备 chip 隐藏,编辑态照常(真实
/// 目录可编辑)。设备浏览位维持「Synshu 设备 ▸ 设备 ▸ 路径」零回归。
///
/// favs 着陆位(V3-3 返工裁定五 §17.1;返工裁定六 §18.1 根段改形):Loc
/// {kind:"favs"}——根段「快捷目录」**即当前位,黑色粗体不可点**(同设备
/// 层「Synshu 设备」形;曾为可点回设备视图)+ 目录 chip(与设备 chip 同
/// 枚渲染器,「目录列表」灰态=devchip-empty,镜像设备层「设备列表」)带
/// 下拉:收件箱置顶项(App 经 favCrumbs.inboxLoc 下发目标 Loc,缺失不列)
/// + 全部快捷目录项(点选直达 fav 位),样式/交互与设备 chip 下拉同款
/// (devmenu);设备 chip 隐藏,搜索/复制路径禁用(着陆页无目录语境)。
/// fav/favdir 位既有形态除根段落点外零回归(§18.1)。
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  IcBack,
  IcCheck,
  IcChevronDown,
  IcChevronRight,
  IcCopyPath,
  IcFolder,
  IcForward,
  IcPc,
  IcPhone,
  IcSearch,
  IcUp,
} from "./icons";
import { favdirAddressDraft, parseAddressInput, type AddressTarget } from "../lib/address";
import type { DeviceInfo } from "../lib/derived";
import { locAtSegment, pathSegments, splitPath, type Loc } from "../lib/nav";

/// 平台图标:android/ios → 手机形(§18.3:ios 曾落 PC 形=bug),其余
/// (windows/linux/macos/未知)→ PC 形。
function PlatformIcon({ platform, size = 14 }: { platform: string | undefined; size?: number }) {
  return platform === "android" || platform === "ios" ? (
    <IcPhone size={size} />
  ) : (
    <IcPc size={size} />
  );
}

/// 快捷命名空间 chip 下拉项(返工裁定六 §18.1 四位统一:favs 着陆/fav/
/// favdir/收件箱):收件箱置顶项(App 下发 inboxLoc 缺席则不列)+ 全部
/// 快捷目录项(点选直达);样式与设备下拉同款 devmenu-item;当前位勾选
/// (收件箱位勾收件箱项、fav/favdir 位勾当前快捷项、着陆位无当前项——槽
/// 位保留空置与设备项对齐)。导出供 SSR 冒烟钉词面/次序/勾选(开合交互
/// 在 AddressBar 内收口)。
export function FavsMenuItems(props: {
  favorites: { id: string; name: string }[];
  inboxAvailable: boolean;
  /// 当前位勾选:收件箱命名空间位=true(收件箱项打勾,且快捷项不打勾)。
  currentInbox: boolean;
  /// 当前位勾选:fav/favdir 位=当前快捷 id(同名项打勾);着陆/收件箱位=""。
  currentFavId: string;
  onPickInbox(): void;
  onPickFav(id: string): void;
}) {
  const { t } = useTranslation();
  return (
    <>
      {props.inboxAvailable && (
        <button
          type="button"
          className="devmenu-item"
          onClick={props.onPickInbox}
          title={t("sidebar.inbox")}
        >
          <span className="devmenu-chk">
            {props.currentInbox ? <IcCheck size={12} /> : null}
          </span>
          <IcFolder size={14} />
          <span className="devmenu-name">{t("sidebar.inbox")}</span>
        </button>
      )}
      {props.favorites.length === 0 && (
        <div className="devmenu-item devmenu-empty">{t("favorites.emptyHint")}</div>
      )}
      {props.favorites.map((f) => (
        <button
          key={f.id}
          type="button"
          className="devmenu-item"
          onClick={() => props.onPickFav(f.id)}
          title={f.name}
        >
          <span className="devmenu-chk">
            {!props.currentInbox && f.id === props.currentFavId ? <IcCheck size={12} /> : null}
          </span>
          <IcFolder size={14} />
          <span className="devmenu-name">{f.name}</span>
        </button>
      ))}
    </>
  );
}

/// 收件箱命名空间命中(§14.3):inboxBase 段前缀匹配(splitPath 显式探测
/// 分隔符——禁手工字符串 startsWith,防 "recv" 误吞 "recvx";尾分隔符等
/// 价形由 splitPath 剥空段吸收)。导出供 rootCrumbLoc 与渲染位同源判定。
export function isInboxPath(path: string, inboxBase: string): boolean {
  if (inboxBase === "") return false;
  const b = splitPath(inboxBase);
  const c = splitPath(path);
  return (
    b.sep === c.sep &&
    b.segs.length <= c.segs.length &&
    b.segs.every((seg, i) => seg === c.segs[i])
  );
}

/// 根段 loc 语义(返工裁定六 §18.1 统一):设备层与 favs 着陆位=当前位
/// (null → 渲染纯文本不可点,「Synshu 设备」/「快捷目录」黑色粗体同形);
/// fav/favdir/收件箱位=favs 着陆位({kind:"favs"}——点击回着陆;favdir 曾
/// 回该快捷混合视图、fav/收件箱曾回设备层,均废);其余 dir 位=设备层
/// ({kind:"devices"})。收件箱命中经 isInboxPath 与渲染位同源。导出供 SSR
/// 钉语义(SSR 面上看不见点击目标)。
export function rootCrumbLoc(current: Loc, inboxBase?: string): Loc | null {
  if (current.kind === "devices" || current.kind === "favs") return null;
  if (current.kind === "favdir" || current.kind === "fav") return { kind: "favs" };
  if (
    current.kind === "dir" &&
    inboxBase != null &&
    inboxBase !== "" &&
    isInboxPath(current.path, inboxBase)
  ) {
    return { kind: "favs" };
  }
  return { kind: "devices" };
}

/// 面包屑 chip 统一渲染器(§18.1):设备 chip 与快捷目录 chip 共用一枚
/// 渲染器——同 devchip 类(+devchip-empty 灰态=「当前位无实体」:设备层
/// 「设备列表」/着陆位「目录列表」互为镜像)、同 chevron、同 aria-expanded
/// 下拉语;仅图标(平台形/文件夹)与词面随命名空间。
function CrumbChip(props: {
  icon: React.ReactNode;
  label: string;
  expanded: boolean;
  empty: boolean;
  onToggle(): void;
}) {
  return (
    <button
      type="button"
      className={"devchip" + (props.empty ? " devchip-empty" : "")}
      onClick={props.onToggle}
      title={props.label}
      aria-expanded={props.expanded}
    >
      {props.icon}
      <span className="devchip-name">{props.label}</span>
      <span className="devchip-chev">
        <IcChevronDown />
      </span>
    </button>
  );
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
  favCrumbs,
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
  /// 快捷命名空间面包屑数据(V3-3 返工 §13.6;§14.3 扩位;§15.5 扩成员;
  /// 返工裁定六 §18.1 chip 统一——四位(fav/favdir/favs/收件箱)恒传全量
  /// 清单 + inboxLoc):清单(id+名+成员集,下拉切换数据源 + 编辑态解析的
  /// 快捷表)、当前快捷 id/名(着陆位/收件箱位中性空)、rel 段(pathSegments
  /// 分解;fav/favs/收件箱位传空)。chip 下拉恒含收件箱置顶项:inboxLoc
  /// =本机收件目录目标 Loc(App 侧以 selfDeviceId+recv_dir 装配;缺失=本
  /// 机收件路径不可用,下拉不列收件箱项)。inboxBase(收件箱 dir 位专用):
  /// 本机 recv_dir 比较基准——App 仅在 dir 位且 current.device===
  /// selfDeviceId 时下发,组件内 isInboxPath 段前缀匹配命中即收件箱命名
  /// 空间位;缺省(undefined)= 旧形态。
  favCrumbs?: {
    favorites: { id: string; name: string; members: { device: string; path: string }[] }[];
    currentFavId: string;
    currentFavName: string;
    relSegments: string[];
    inboxBase?: string;
    inboxLoc?: Loc | null;
  };
}) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [devOpen, setDevOpen] = useState(false);
  // 快捷 chip 下拉(§18.1:fav/favdir/favs/收件箱四位共有;与设备下拉
  // 互斥语义同源)。
  const [favOpen, setFavOpen] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const crumbboxRef = useRef<HTMLDivElement>(null);

  const isDir = current.kind === "dir";
  const isFavdir = current.kind === "favdir";
  // fav 位(混合视图):§14.3 起入快捷命名空间——根段「快捷目录」+ fav
  // chip 带下拉(补齐 R8 无名段缺口);设备 chip 隐藏、编辑禁入不变。
  const isFav = current.kind === "fav";
  // favs 着陆位(返工裁定五 §17.1;§18.1 根段改形):根段即当前位不可点
  // + 目录 chip(「目录列表」灰态)带下拉(收件箱置顶 + 全部快捷目录)。
  const isFavs = current.kind === "favs";
  // 收件箱命名空间位(§14.3):本机 recv_dir 的 dir 位。段前缀匹配判定
  // (isInboxPath:splitPath 显式探测分隔符,nav 纪律——禁手工字符串
  // startsWith,防 "recv" 误吞 "recvx";尾分隔符等价形由 splitPath 剥空
  // 段吸收);path===recv_dir(段数相等)或其下级命中。selfDeviceId 门在
  // App 侧(仅本机 recv_dir 位才经 favCrumbs.inboxBase 下发基准)。
  const inboxBase = favCrumbs?.inboxBase;
  const isInbox =
    isDir && inboxBase != null && inboxBase !== "" && isInboxPath(current.path, inboxBase);

  /// 面包屑数据(trail = 根段 ▸ chip ▸ 路径段):根段恒在场——设备层与
  /// favs 着陆位(§18.1)即当前位不可点(黑色粗体同形);fav/favdir/收件
  /// 箱位根段=「快捷目录」可点回着陆位(§18.1 统一:favdir 曾回该快捷混
  /// 合视图、fav/收件箱曾回设备视图,均废;语义收口在 rootCrumbLoc 导出
  /// 纯函数);目录位根段=「Synshu 设备」可点回设备层(非传输落点,渲染
  /// 侧不挂 data-drop-loc)。设备名不设独立 crumb,由设备 chip 承担(设备
  /// 根时 chip 即当前位)。路径段沿用现有逻辑:pathSegments 数据源、
  /// locAtSegment 点击导航、末段降纯文本不可点;设备根(path="")无路径段。
  const favsNs = current.kind === "favdir" || current.kind === "fav" || isFavs || isInbox;
  const rootCrumb: Crumb = favsNs
    ? { label: t("address.favRoot"), loc: rootCrumbLoc(current, inboxBase) }
    : { label: t("sidebar.devices"), loc: rootCrumbLoc(current, inboxBase) };
  const pathCrumbs: Crumb[] = [];
  if (isDir && current.path !== "") {
    if (isInbox && inboxBase != null) {
      // 收件箱命名空间段(§18.1:收件箱=chip 当前值,不再作首段):仅
      // recv_dir 之下余段(照常 locAtSegment 全路径段计数导航;末段=当前
      // 位降纯文本不可点;收件箱根位无余段=零段)。
      const b = splitPath(inboxBase);
      const c = splitPath(current.path);
      c.segs.slice(b.segs.length).forEach((label, j) => {
        pathCrumbs.push({
          label,
          loc: locAtSegment(current.device, current.path, b.segs.length + j + 1),
        });
      });
    } else {
      pathSegments(current.path).forEach((label, i) => {
        pathCrumbs.push({ label, loc: locAtSegment(current.device, current.path, i + 1) });
      });
    }
    // 末段即当前位:降为纯文本,不可点(收件箱根位无段,跳过)。
    if (pathCrumbs.length > 0) pathCrumbs[pathCrumbs.length - 1].loc = null;
  }

  /// 进入编辑态:预填当前完整原生路径(目录位;设备层/卷列表位预填空串)
  /// 并全选(effect 见下);模态占线或已在编辑时不进入,**设备层同样可进**
  /// (§7 规则 2 的 needDeviceOrAbsolute 分支即由设备层编辑触发);进入
  /// 同时收起设备下拉。**favdir 位 §15.5 起启用**:预填快捷命名空间虚拟
  /// 地址(`快捷目录\{名}\{rel}`——favdirAddressDraft 生产侧,非原始设备
  /// 路径;favCrumbs 缺失的防御不进入,理论不达);**fav 位仍禁**(R8 对
  /// 齐,§15.5 维持:混合视图无单一原生路径,F4/Ctrl+L/点空白三入口共用
  /// 本门,一处禁全禁)。
  const beginEdit = useCallback(() => {
    if (modalBlocked || editing || current.kind === "fav") return;
    if (current.kind === "favdir") {
      if (favCrumbs == null) return; // 防御:命名空间数据缺失不进(理论不达)
      setDevOpen(false);
      setFavOpen(false);
      setDraft(favdirAddressDraft(favCrumbs.currentFavName, favCrumbs.relSegments));
      setEditing(true);
      return;
    }
    setDevOpen(false);
    setFavOpen(false);
    setDraft(current.kind === "dir" ? current.path : "");
    setEditing(true);
  }, [current, modalBlocked, editing, favCrumbs]);

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

  /// 导航换位:编辑态还原、设备/fav 两下拉收起(导航必换 current 对象身份)。
  useEffect(() => {
    setEditing(false);
    setDevOpen(false);
    setFavOpen(false);
  }, [current]);

  /// 下拉(设备/fav)的外点/Esc 收起(任一打开期间挂 document 级监听)。
  useEffect(() => {
    if (!devOpen && !favOpen) return;
    const onDown = (e: PointerEvent) => {
      if (crumbboxRef.current?.contains(e.target as Node)) return;
      setDevOpen(false);
      setFavOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setDevOpen(false);
        setFavOpen(false);
      }
    };
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [devOpen, favOpen]);

  /// 编辑态回车提交:空/纯空白视同取消(还原,不发解析——文档化裁定);
  /// 否则 parseAddressInput(纯函数,lib/address.ts)解析后上交并还原。
  /// 设备层以 current=null 参与解析:带设备前缀直达目标,无前缀(相对或
  /// 绝对形)得 needDeviceOrAbsolute 由 App toast(spec §7 规则 2);favdir
  /// 位(§15.5)同 current=null——快捷命名空间语法直达 fav/favdir,设备
  /// 前缀形直达原生目标,无前缀裸路径同样 needDeviceOrAbsolute(命名空间
  /// 位不提供「当前设备相对路径」语境,虚拟地址即其路径语形)。
  const submitDraft = () => {
    const s = draft.trim();
    setEditing(false);
    if (s === "") return;
    onAddressSubmit(
      parseAddressInput(
        s,
        devices,
        current.kind === "dir" ? { device: current.device, path: current.path } : null,
        favCrumbs?.favorites ?? [],
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

  /// fav 下拉选中快捷目录 = 回该快捷混合视图首层(返工 §13.6 裁定;
  /// 含当前项——即从命名空间内直接回混合视图)。
  const pickFav = (id: string) => {
    setFavOpen(false);
    onNavigate({ kind: "fav", id });
  };

  /// 快捷 chip 下拉点收件箱(§18.1 四位统一;返工裁定五 §17.2 起源):直
  /// 达本机收件目录浏览(目标 Loc 由 App 经 favCrumbs.inboxLoc 下发;缺失
  /// =本机收件路径不可用,项本就不渲染,此为防御)。
  const pickInbox = () => {
    setFavOpen(false);
    const loc = favCrumbs?.inboxLoc;
    if (loc != null) onNavigate(loc);
  };

  /// 面包屑段的拖放落点键:目录位且非卷列表段才可落(device\0path)。
  const dropKey = (loc: Loc | null): string | undefined =>
    loc != null && loc.kind === "dir" && loc.path !== ""
      ? `${loc.device}\u0000${loc.path}`
      : undefined;

  /// favdir rel 段面包屑(返工 §13.6):数据源 favCrumbs.relSegments
  /// (App 经 pathSegments 分解);第 k 段点击 → 同命名空间内 rel 前 k 段
  /// (分隔符取 current.rel 经 splitPath 显式探测——nav 纪律);末段即当前
  /// 位降纯文本不可点。命名空间段不挂 data-drop-loc(设备/原始路径在
  /// 此位隐藏,落点由主区宿主 viewDropKey 承担)。
  const favRelCrumbs: Crumb[] = [];
  if (current.kind === "favdir" && favCrumbs != null) {
    const fd = current;
    const segs = favCrumbs.relSegments;
    const { sep } = splitPath(fd.rel);
    segs.forEach((label, i) => {
      favRelCrumbs.push({
        label,
        loc:
          i === segs.length - 1
            ? null
            : {
                kind: "favdir",
                favId: fd.favId,
                device: fd.device,
                base: fd.base,
                rel: segs.slice(0, i + 1).join(sep),
              },
      });
    });
  }

  /// 当前浏览设备快照(chip 平台图标/下拉勾选;设备层或快照缺失为
  /// undefined,图标落 PC 形缺省)。
  const curDevice =
    isDir
      ? devices.find((d) => d.device_id === current.device)
      : undefined;

  /// 第二级槽位(§18.1 统一):fav/favdir/favs/收件箱四位且有数据 → 快捷
  /// chip(与设备 chip 同一枚渲染器/devchip 同款:着陆位=「目录列表」灰态,
  /// 镜像设备层「设备列表」;fav/favdir=当前快捷名;收件箱位=「收件箱」——
  /// 收件箱自此为 chip 合法当前值,旧「无下拉裸段」特殊形废止);fav 位数
  /// 据缺失的防御不渲染(理论不达——App 恒传);favdir 位数据缺失的防御仍
  /// 落设备 chip(原样保留);其余位设备 chip 照常。
  const favChipShown = (isFavdir || isFav || isFavs || isInbox) && favCrumbs != null;
  const devChipShown = !favChipShown && !isFav && !isInbox;

  /// 复制路径禁用面:设备层与卷列表位(path="",无原生路径可复制);
  /// favdir 位可用——App 给的是成员原始绝对路径(base+rel,返工 §13.6
  /// Review Focus #2 裁定:命名空间不破复制路径既有功能)。
  const copyDisabled = !(isDir || isFavdir) || (isDir && current.path === "");

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
            {/* 根段(§18.1):设备层与 favs 着陆位为当前位(粗体不可点,
                「Synshu 设备」/「快捷目录」同形);设备目录位=「Synshu 设备」
                可点回设备层(非传输落点,不挂 data-drop-loc);fav/favdir/
                收件箱位=「快捷目录」可点回 favs 着陆位(语义收口在
                rootCrumbLoc 导出纯函数)。 */}
            <span className="crumb-item">
              {rootCrumb.loc == null ? (
                <span className="crumb cur">{rootCrumb.label}</span>
              ) : (
                <button
                  type="button"
                  className="crumb crumb-link"
                  onClick={() => onNavigate(rootCrumb.loc as Loc)}
                  title={rootCrumb.label}
                >
                  {rootCrumb.label}
                </button>
              )}
            </span>
            {/* 根段▸第二级分隔符:第二级 chip 在场才渲染(chip 即第二级,
                §18.1——收件箱裸段旧形废止,随之不再有「无 chip 有分隔符」位)。 */}
            {(favChipShown || devChipShown) && (
              <span className="csep">
                <IcChevronRight size={11} />
              </span>
            )}
            {favChipShown ? (
              <>
                {/* 快捷 chip(§18.1 四位统一,与设备 chip 同一枚渲染器):
                    着陆位=「目录列表」灰态(镜像设备层「设备列表」),fav/
                    favdir=当前快捷名,收件箱位=「收件箱」;点击弹清单下拉
                    (收件箱置顶 + 全部快捷目录,当前位勾选;设备 chip 在此
                    四位隐藏——设备与原始路径不入面包屑);rel 段仅 favdir
                    位有(fav/favs/收件箱位 relSegments 传空或无关,
                    favRelCrumbs 恒空数组) */}
                <CrumbChip
                  icon={<IcFolder size={14} />}
                  label={
                    isInbox ? t("sidebar.inbox") : isFavs ? t("address.dirList") : favCrumbs.currentFavName
                  }
                  expanded={favOpen}
                  empty={isFavs}
                  onToggle={() => setFavOpen((o) => !o)}
                />
                {favRelCrumbs.map((c, i) => (
                  <span key={i} className="crumb-item">
                    <span className="csep">
                      <IcChevronRight size={11} />
                    </span>
                    {c.loc == null ? (
                      <span className="crumb cur">{c.label}</span>
                    ) : (
                      <button
                        type="button"
                        className="crumb crumb-link"
                        onClick={() => onNavigate(c.loc as Loc)}
                        title={c.label}
                      >
                        {c.label}
                      </button>
                    )}
                  </span>
                ))}
              </>
            ) : devChipShown ? (
              /* 设备 chip:同一枚渲染器(§18.1 统一);fav/favs/收件箱位
                 不渲染;favdir 位 favCrumbs 缺失的防御分支仍落此处渲染
                 (理论不达,原样保留)。 */
              <CrumbChip
                icon={<PlatformIcon platform={curDevice?.platform} />}
                label={isDir ? deviceName(current.device) : t("address.deviceList")}
                expanded={devOpen}
                empty={current.kind === "devices"}
                onToggle={() => setDevOpen((o) => !o)}
              />
            ) : null}
            {pathCrumbs.map((c, i) => (
              <span key={i} className="crumb-item">
                <span className="csep">
                  <IcChevronRight size={11} />
                </span>
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
        {devOpen && !editing && !isFavdir && !isFav && (
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
        {favOpen && !editing && favCrumbs != null && (
          /* 快捷 chip 下拉(§18.1 四位统一):收件箱置顶(App 经 inboxLoc
             下发,缺失不列)+ 全部快捷目录;当前位勾选(收件箱位勾收件箱、
             fav/favdir 位勾当前快捷、着陆位无勾选);点选直达(快捷=回混
             合视图首层,收件箱=recv_dir 浏览)。 */
          <div className="devmenu" role="menu">
            <FavsMenuItems
              favorites={favCrumbs.favorites}
              inboxAvailable={favCrumbs.inboxLoc != null}
              currentInbox={isInbox}
              currentFavId={favCrumbs.currentFavId}
              onPickInbox={pickInbox}
              onPickFav={pickFav}
            />
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
          disabled={!isDir && !isFavdir && !isFav}
          value={searchQ}
          onChange={(e) => onSearchChange(e.target.value)}
          placeholder={t("address.searchPlaceholder")}
          aria-label={t("address.searchPlaceholder")}
        />
      </span>
    </div>
  );
}
