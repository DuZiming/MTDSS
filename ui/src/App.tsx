/// MTDSS 主窗口壳(M3b/M3c):工具栏(排序/查看/操作按钮)/ 地址栏
/// (导航+设备 chip+面包屑+复制路径+可编辑路径+搜索)/ 左侧树 /
/// 设备列表或目录三视图(详细信息|列表|平铺)/ 未登录引导态 / 离线提示
/// 条 / 状态栏。
///
/// 数据面:启动时拉 hello/status/devices;冷启动下这组初始拉取可能失败
/// (daemon 被桥拉起尚需 1-2s),故另订阅 daemon://connected——泵每次
/// (重)连成功都发,收到即重拉全量并回到设备视图。此后靠 daemon 事件
/// (daemon://online、daemon://devices、daemon://login)保持新鲜。
///
/// 导航面:NavState(Loc 历史栈)承载 后退/前进/上级;目录视图按当前
/// Loc 拉取列表,seqRef + applyListing 守卫丢弃过期响应;登出/断连全量
/// 复位(回到设备视图,清空浏览状态)。
///
/// 浏览面:行序 = filterEntries(搜索)∘ sortEntries(排序),在 App 统一
/// 计算后下发三视图(状态栏计数/Shift 范围同源);选择集与锚点由
/// applyClickSelect 解析(单击/Ctrl/Shift);当前浏览设备转离线时保留
/// 内容 + 顶部提示条 + canEnterDir 阻断进入子目录。
///
/// 操作面 I(Task 11):右键菜单、框选、工具栏操作按钮按选中态启用、
/// Del 键删除。删除 = 回收优先直接 RPC(spec §4.3,可回收静默);「回收
/// 站不可用」标记 → 永久删除确认 → force 整批重发(确认批携带设备快照,
/// 防导航换设备后 force 误删);code 3 → 「设备离线」提示;其余失败 →
/// 瞬时提示条,成功(或部分成功)后刷新。
///
/// 操作面 II(Task 12;M4b 移交债①):内部剪贴板(剪切/复制 →
/// {device, entries, cut};MTDSS 不写系统剪贴板,MTDSS→Explorer 粘贴为
/// V1 反目标)与粘贴分派(pastePlan 五态:upload/download/remote_copy/
/// device_copy/local_copy——本机→本机回迁 local_copy 作业面,M4b 起带
/// skip/overwrite 策略参数,粘贴/拖拽得作业卡、进度与跳过计数;曾因无
/// 策略参数致同机冲突对话框不可达而退役(M4a Defect C,临时改派
/// device_copy),策略化后回归;远端同设备仍派 device_copy(ctrl
/// copy/move,daemon 侧 device==self 短路直调本地 fs);空内部剪贴板 →
/// 桥读系统 CF_HDROP → upload);冲突预扫(源条目 × 目标 list,目录恒
/// 合并)→ 四键冲突对话框(全部键只作用剩余未决项)→ transfer.start 携
/// skip/overwrite,同设备面(Task 4)同款预扫(planSameDeviceOp)→ 对
/// 话框,裁决经判定键换算(ctrl copy/move 与 local_copy 均携换算后
/// skip/overwrite);「下载到本机…」= 目录选择器 →
/// download 作业;拖入 = Tauri onDragDropEvent(dragDropEnabled,外部文件
/// 落真实路径)→ upload;**窗口内拖拽与拖出共用一套指针拖拽**(见下)。
///
/// 快捷位动作对等与添加快捷(V3-3 Task 11):目录位右键「添加到快捷
/// 目录」(favAddTargets 门:目录行或全目录多选 → FavPickerDialog 选快捷
/// 落 addMembers;「新建」流经命名对话框,创建成功按 id 差集认领新条目
/// 直加成员并双层关闭);fav 位行右键(favCtx 标记)与选择装配同款菜单
/// ——打开(钻取 dir 位)/下载到本机(跨源分组逐组 download,本机组跳
/// 过;冲突挂起即停,余组计数提示)/复制·剪切(单源组门,跨源 toast 阻
/// 断)/删除(多组回收梯:
/// favDel 独立确认实例,need_force 组确认后逐组 force);Del/Ctrl+X/C 与
/// 工具栏操作在 fav 位路由到同一批 mixed 处理器(粘贴 T12 同语义:单成
/// 员直进/多成员开「存入哪个成员」选择层)。
///
/// 拖放三路(V3-3 Task 12):fav 位行可拖(选中集跨源按设备分组,与
/// 目录行同一套指针拖拽机制);落点键两形——既有 `device\0path`(树节点/
/// 面包屑/当前目录宿主/快捷分组节头/收件箱行)逐源组 fan-out,新增
/// `fav\0<id>`(侧栏快捷行/快捷视图宿主)经 resolveFavDropTarget 单成员
/// 直落/多成员开「存入哪个成员」选择层;Explorer 外部拖入在快捷位经命中
/// 测试定落点(物理→CSS 像素换算);粘贴进快捷同 resolveFavDropTarget
/// 语义(Ctrl+V/工具栏/菜单共用,canPaste fav 位=有成员)。
///
/// 快捷管理面(V3-3 Task 13):侧栏快捷行右键 TreeNodeMenu(打开/重命名/
/// 管理成员/删除快捷;目标本体从 store 现值解析,他端删除随 data_changed
/// 同步后面自然收口);重命名复用 FavNameDialog(initial 预填切重命名态,
/// renameFav 成功名随 store 下发);管理屏 FavManageDialog 成员逐行移除
/// (成功后正浏览该快捷则 bumpIfViewing 硬重种子——T9 遗留收口);删除=
/// 只解除链接确认梯(deleteFav,绝不触碰远端文件),成功后正浏览该快捷
/// 则回设备视图。
///
/// 传输面(Task 13,Task 9 抽屉化):event.transfer 事件经
/// applyTransferEvents 收敛进作业 Map(乱序/重复/终态后迟到进度在 reducer
/// 收口);TransferDrawer 于窗底常驻(spec §7):收起=细摘要条(进行中/
/// 速率/中断/失败),展开=六列表格持久历史面(暂停⇄继续/取消/重新发起/
/// 清除,筛选四档,开合与拖高记忆在 localStorage),右下浮卡 TransferCards
/// 退役;连接种子 = transfer.list 全量重置(终态行是历史面照显示,可见性
/// 收敛全在抽屉);状态栏速率段 = aggRate(仅 running 计速,非终态计数)。
/// event.notify → 窗内 toast 回显(OS 通知由 daemon 自发,UI 不重复弹;
/// 5s 自隐,队列上限 3)。控制 RPC(pause/resume/cancel/reinitiate/clear)
/// 错误统一转 toast(remote_copy 拒 pause 即此呈现);reinitiate/clear 成功
/// 后全量重拉(翻新换 id / 记录摘除即刻反映进抽屉)。
///
/// 打磨四小件(Task 13):树节点懒加载 spinner;窗口 focus 重拉 devices
/// (快照陈旧配套);树节点文本超界省略(样式);Shift 范围行以更深的
/// shift-range 底色与普通选中区分。
///
/// 地址栏重设计(P4b Task 4,spec §7):设备 chip 固定+全部设备下拉
/// (在线可选/离线灰显/当前勾选;选中=进设备根入历史);复制路径 =
/// daemonCopyText(navigator.clipboard 优先、桥 copy_text 兜底)成功轻
/// toast,设备层与卷列表位禁用;编辑态(点面包屑空白/F4/Ctrl+L,模态
/// 占线不进入)预填原生全路径全选,回车经 parseAddressInput(lib/address
/// 纯函数)解析——nav/deviceRoot 转 navigate,error 转 toast;Esc/失焦
/// 还原。解析失败不跳转,目标不存在/离线由浏览错误态自然呈现。
///
/// 更新面(Task 15):event.update 经 updateStateFromEvents 收敛进快照
/// (乱序旧帧/终态粘滞/failed 保错在 reducer 收口);available 翻转即
/// pushToast「发现新版本 X」(「查看」动作直达更新页,不自动出队、5s
/// TTL 照常)。更新页(UpdatePage 覆盖层)独立于登录态、不进 nav 栈,
/// updatePageOpen 挂载:页面挂载时拉一次 update.state 对账;单实例路由
/// (托盘「检查更新…」拉起 ui.exe --update-page → daemon://open-update)
/// 与冷启动标记(take_open_update_request)开页。check/apply RPC 经
/// runUpdateRpc 统一执行:返回的状态 JSON 与事件同形走同一 reducer 入口;
/// 失败(含安装序列中连接断开)→ 重拉 update.state 兜底,快照是唯一
/// 事实源,失败态文案随 failed 帧呈现。
///
/// 快捷命名空间位(V3-3 返工 §13.6):Loc 增 `{kind:"favdir"; favId;
/// device; base; rel}`(rel="" = 成员根)。钻取三入口(混合视图双击/菜单
/// 打开 = openFavDir 按最深包含成员 memberOfPath 定 base;侧栏成员节点 =
/// openFavDirMember)全进命名空间——设备名与原始路径在面包屑隐藏,「上一
/// 级」在虚拟路径内逐级,顶级回混合视图。主区=普通目录三视图 as-dir:取数
/// 复用 dir 位 effect(dirKey=device\0joinRel(base,rel)),行=path 粒度,
/// 选择/右键/键盘/粘贴/拖放(含 Explorer 拖入)零新动作码,落点键=当前
/// 命名空间原生路径;复制路径给 base+rel 原始绝对路径;编辑态禁用;离线
/// 门/状态栏/工具栏操作可用性同 dir 位。
///
/// 快捷目录着陆位(V3-3 返工裁定五 §17.1/§17.2):Loc 增 {kind:"favs"}
/// ——侧栏「快捷目录」组头行点击落此位(§15.2 落收件箱裁定废止),右侧
/// 渲染着陆页 FavsLandingView(收件箱置顶行 + 全部快捷目录行,双击/回车
/// 进分组视图);面包屑根段「快捷目录」可点回设备层 + fav chip 下拉
/// (收件箱置顶 + 各快捷目录,点选直达,样式同设备下拉);上级=设备层;
/// 状态栏计数/选择/拖放与设备层同形(着陆页非传输落点)。树收件箱行
/// caret 可展开本机收件目录子级(Sidebar 内 DirNode 复用)。
///
/// 拖拽实现注(为何不用 HTML5 DnD):wry 在 Windows 上为拦截外部文件
/// 落点(dragDropEnabled=true,默认)会把 WebView2 子窗口的 OLE IDropTarget
/// 替换为自己的——这会**杀死页面内 HTML5 拖放**(dragover/drop 不再到达
/// 页面,Tauri 已知限制)。因此窗口内拖拽改为指针事件手写:行 mousedown
/// 越 4px 阈值 → body.setPointerCapture(Chromium 捕获语义保证拖出窗口
/// 后仍收 move/up)→ 悬停命中 [data-drop-loc](树节点/面包屑/当前目录)
/// 高亮 → 松开即 dragPlan 分派;松开在窗口外 → drop_resolve_at_cursor
/// 解析桌面/资源管理器目录 → download 直落;unresolvable → 钉死提示。
/// 拖拽永不提供 OLE 数据(反目标:让系统资源管理器经手数据)。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { getCurrentWindow } from "@tauri-apps/api/window";

import AddressBar from "./components/AddressBar";
import ApprovalModal from "./components/ApprovalModal";
import ConflictDialog from "./components/ConflictDialog";
import ContextMenu, { type CtxMenuState } from "./components/ContextMenu";
import ConfirmDialog from "./components/ConfirmDialog";
import DeviceTable from "./components/DeviceTable";
import FavManageDialog from "./components/FavManageDialog";
import FavMemberPicker from "./components/FavMemberPicker";
import FavNameDialog from "./components/FavNameDialog";
import FavPickerDialog from "./components/FavPickerDialog";
import FavView from "./components/FavView";
import FavsLandingView from "./components/FavsLandingView";
import FileDetails from "./components/FileDetails";
import FileList, { EntryIcon, type DirViewProps } from "./components/FileList";
import FileTiles from "./components/FileTiles";
import GuideView from "./components/GuideView";
import type { ViewMode } from "./components/Toolbar";
import Sidebar from "./components/Sidebar";
import StatusBar from "./components/StatusBar";
import Toolbar from "./components/Toolbar";
import TransferDrawer from "./components/TransferDrawer";
import TreeNodeMenu, { type TreeNodeMenuItem } from "./components/TreeNodeMenu";
import { NotifyToasts, type ToastItem } from "./components/NotifyToasts";
import UpdatePage from "./components/UpdatePage";
import { pastePlan, type PastePlan } from "./lib/clipboard";
import type { AddressTarget } from "./lib/address";
import {
  applyListing,
  canEnterDir,
  filterEntries,
  headerClick,
  sortEntries,
  type FsEntry,
  type Listing,
  type SortState,
} from "./lib/browse";
import { zhCmp } from "./lib/collate";
import { modalBusy } from "./lib/modality";
import { loadTheme, nextTheme, saveTheme, type ThemeId } from "./lib/theme";
import {
  approvalRespond,
  clipboardFiles,
  daemonCopy,
  daemonCopyText,
  daemonDelete,
  daemonDevices,
  daemonHello,
  daemonList,
  daemonMove,
  daemonStat,
  daemonStatus,
  daemonTransferCancel,
  daemonTransferClear,
  daemonTransferList,
  daemonTransferPause,
  daemonTransferReinitiate,
  daemonTransferResume,
  daemonTransferStart,
  dropResolveAtCursor,
  friendlyError,
  kind,
  listenApproval,
  listenConnected,
  listenDeviceEvicted,
  listenDevices,
  listenLogin,
  listenNotify,
  listenOnline,
  listenOpenUpdate,
  listenTransfer,
  listenUpdate,
  takeOpenUpdateRequest,
  updateApply,
  updateCheck,
  updateState,
  pickFolder,
  toRpcError,
  type ApprovalEvent,
  type DropResolveResult,
  type HelloInfo,
  type StatusInfo,
  type TransferStartParams,
} from "./lib/daemon";
import type { DeviceInfo } from "./lib/derived";
import { clearFavCache } from "./lib/favfetch";
import { favAddTargets, favdirLocAt, memberKey, resolveFavDropTarget, type FavMember, type Favorite } from "./lib/favorites";
import { useFavorites } from "./lib/favstore";
import { downloadMixedPlans, groupRowsByDevice, planMultiSourceDrop, runGroupsWithConflictPause, type DragSourceSel, type MixedRow } from "./lib/mixedview";
import {
  joinRel,
  navBack,
  navForward,
  navGo,
  parentLoc,
  pathSegments,
  relUnderBase,
  type Loc,
  type NavState,
} from "./lib/nav";
import {
  applyClickSelect,
  clickMode,
  rubberBandSelect,
  type RowRect,
} from "./lib/select";
import { scanConflicts, type Conflict } from "./lib/conflict";
import { localCopyStartParams, planSameDeviceOp, type SameDeviceArgs } from "./lib/paste";
import { updateStateFromEvents, type UpdateEvent, type UpdateSnapshot } from "./lib/update";
import {
  aggRate,
  applyTransferEvents,
  seedJobs,
  type JobSnapUI,
} from "./lib/transfer";
import "./App.css";

/// 浏览列表的加载三态(错误时不保留旧列表,条目清空只留错误与重试)。
interface BrowseState {
  loading: boolean;
  listing: Listing | null;
  error: string | null;
}

const BROWSE_IDLE: BrowseState = { loading: false, listing: null, error: null };

/// 初始快照:hello(版本/登录态)+ status(在线态)+ devices(目录)。
async function loadAll(
  setHello: (h: HelloInfo | null) => void,
  setStatus: (s: StatusInfo | null) => void,
  setDevices: (d: DeviceInfo[]) => void,
) {
  try {
    setHello(await daemonHello());
  } catch {
    setHello(null); // daemon 未就绪:引导态兜底,等 daemon://connected 信号重拉
  }
  try {
    setStatus(await daemonStatus());
  } catch {
    setStatus(null);
  }
  try {
    setDevices((await daemonDevices()).devices);
  } catch {
    setDevices([]);
  }
}

/// 两位相等:设备层与设备层相等;快捷着陆位与快捷着陆位相等(返工裁定
/// 五 §17.1);目录位比较设备 + 路径;快捷位比较 id;快捷命名空间位
/// (V3-3 返工 §13.6)四字段全比(favId/device/base/rel——同快捷同成员
/// 下 rel 逐级各成一位)。
function sameLoc(a: Loc, b: Loc): boolean {
  if (a.kind !== b.kind) return false;
  if (a.kind === "dir" && b.kind === "dir") return a.device === b.device && a.path === b.path;
  if (a.kind === "fav" && b.kind === "fav") return a.id === b.id;
  if (a.kind === "favs" && b.kind === "favs") return true;
  if (a.kind === "favdir" && b.kind === "favdir") {
    return (
      a.favId === b.favId &&
      a.device === b.device &&
      a.base === b.base &&
      a.rel === b.rel
    );
  }
  return true;
}

export default function App() {
  const { t } = useTranslation();
  const [hello, setHello] = useState<HelloInfo | null>(null);
  const [status, setStatus] = useState<StatusInfo | null>(null);
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  // 导航历史:current 初始为设备层(顶层视图)。
  const [nav, setNav] = useState<NavState>({
    back: [],
    fwd: [],
    current: { kind: "devices" },
  });
  // 目录列表排序状态(P4b Task 5:键+方向,默认名称升序;工具栏「排序」
  // 菜单两组 radio 切换,T6 起表头点击同源,仅内存态)。
  const [sortState, setSortState] = useState<SortState>({ key: "name", dir: "asc" });
  // 内容区视图模式(工具栏「查看」菜单切换,默认详细信息,仅内存态)。
  const [viewMode, setViewMode] = useState<ViewMode>("details");
  // 快捷位组织偏好已废止(返工波 4 §16.1 分类恒定:混合|分组切换与
  // localStorage 偏好删除,主区恒分组)。
  // 主题(P4b Task 1):三主题循环,持久化 localStorage。首帧由 main.tsx
  // 挂载前同步落 data-theme(无闪烁),此处经同一 loadTheme 同源起步。
  const [theme, setTheme] = useState<ThemeId>(loadTheme());
  // 搜索词(地址栏输入;filterEntries 客户端过滤,导航换目录即清空)。
  const [searchQ, setSearchQ] = useState("");
  // 当前行选择:路径集合(单击单选 / Ctrl 增减 / Shift 范围)。
  const [selection, setSelection] = useState<Set<string>>(new Set());
  // Shift 范围选择的锚点路径(最近一次非 Shift 点击的行)。
  const [anchorPath, setAnchorPath] = useState<string | null>(null);
  // 当前目录列表(仅 kind==="dir" 时有意义)。
  const [browse, setBrowse] = useState<BrowseState>(BROWSE_IDLE);
  // 重试计数:变化即对当前目录重发列表请求。
  const [reloadTick, setReloadTick] = useState(0);
  // 列表响应序号:单调递增,过期响应经 applyListing 丢弃。
  const seqRef = useRef(0);
  // ---- 操作面(Task 11)瞬时态 ----
  // 右键菜单打开态(触发点坐标 + 命中条目;null = 收起)。
  const [ctxMenu, setCtxMenu] = useState<CtxMenuState | null>(null);
  // 框选覆盖层矩形(client 坐标;null = 未在框选)。
  const [bandRect, setBandRect] = useState<
    { left: number; top: number; width: number; height: number } | null
  >(null);
  // 「回收站不可用 → 永久删除」确认梯的待确认批次。**携带 device 快照**
  // (Task 12 MUST-FIX,Task 11 评审遗留):弹窗无焦点陷阱,键盘在错误条
  // 与确认框间游走期间导航可换设备;若只存 paths 而确认时才从 nav.current
  // 取设备,会把旧设备的路径整批 force 删到新设备上。开梯时定死设备,
  // confirmForceDelete 按快照重发,与导航状态彻底解耦。
  const [confirmDel, setConfirmDel] = useState<{ device: string; paths: string[] } | null>(null);
  // 瞬时操作提示(删除失败/设备离线/传输已开始等;null = 无)。自动消隐。
  // tone: error = 红(失败/阻断);info = 蓝(已开始等中性反馈)。
  const [notice, setNotice] = useState<{ text: string; tone: "error" | "info" } | null>(null);
  // 三视图宿主(框选的行测量根与拖拽启动面)。
  const viewHostRef = useRef<HTMLDivElement>(null);
  // 删除在途防重入(Del 键连按/菜单与工具栏并发触发)。
  const deletingRef = useRef(false);
  // 本次框选拖拽已越阈标记:用于吞掉 mouseup 后落在空白处的 click 清空,
  // 避免框选结果被随后的空白点击清掉。
  const bandedRef = useRef(false);
  // 本次指针拖拽(窗口内拖/拖出)已成立标记:捕获相 click 监听据此吞掉
  // 松开后的 click(防拖拽末尾的 click 重选/清选),见下方拖拽实现注。
  const dragMovedRef = useRef(false);

  // ---- 操作面 II(Task 12)状态 ----
  // 内部剪贴板:device + 完整条目元数据(冲突预扫免二次 stat)+ cut。
  // MTDSS 复制不写系统剪贴板(MTDSS→Explorer 粘贴为 V1 反目标)。
  const [clip, setClip] = useState<{ device: string; entries: FsEntry[]; cut: boolean } | null>(null);
  // 冲突对话框挂起批:预扫出的冲突行 + 决策落定后的执行闭包(跨设备
  // 传输面与同机 copy/move 面 Task 4 共用同一对话框)。
  const [pendingConflict, setPendingConflict] = useState<{
    conflicts: Conflict[];
    exec: (policy: { skip: string[]; overwrite: string[] }) => void;
  } | null>(null);
  // 冲突挂起标记(T11 评审 Important #1):runTransfer 预扫命中冲突即置
  // 位,伴生 pendingConflict 的 ref——downloadMixed 的异步循环闭包里读
  // state 恒旧,判停必须走 ref;每次分组发起前复位。downloadMixed 据此
  // break + 显式计数提示,把「多组同时冲突静默丢组」变可见;单组冲突/
  // 无冲突/一组冲突一组直发三条已正确路径不受影响(rest=0 不提示)。
  const conflictParkedRef = useRef(false);
  // 指针拖拽会话(ref 不触发渲染):越阈后进入;悬停高亮/拖拽中光标
  // 另用 state。outside = 指针当前在窗口外(拖出中)。sources = 源组集
  // (V3-3 T12 多源化):dir 位起步恒单组(srcDir=当前目录,原地放置
  // 判定需要);fav 位行拖拽按来源设备分组多组(srcDir=null)。
  const dragRef = useRef<{
    pointerId: number;
    startX: number;
    startY: number;
    sources: DragSourceSel[];
    started: boolean;
    outside: boolean;
  } | null>(null);
  // 拖拽悬停高亮的落点键(device\0path;null = 无悬停/未拖拽)。
  const [dropHoverKey, setDropHoverKey] = useState<string | null>(null);
  // 拖拽进行中(grabbing 光标与视觉反馈)。
  const [dragging, setDragging] = useState(false);
  // ---- 拖拽视觉(Task 9,spec §9;机制零改动,只补观感) ----
  // ghost 浮卡内容:拖拽成立时一次定死(首项条目 + 总数;拖拽期间不变,
  // 挂载即渲染,位置不走 state)。首项图标复用三视图 EntryIcon(目录
  // IcFolder / 文件类型角标页形,同一分支裁定)。
  const [dragGhost, setDragGhost] = useState<{ entry: FsEntry; count: number } | null>(null);
  // 正被拖拽的行集(下发三视图挂 .drag-source 半透明;null = 无拖拽)。
  // 键形随会话起步位(T12):dir 位 = 路径集,fav 位 = 复合键集(行
  // rowKey = device\0path),视图 has(key) 同键比对。
  const [dragSourcePaths, setDragSourcePaths] = useState<ReadonlySet<string> | null>(null);
  // ghost 定位走「廉价路径」(不逐 move 重渲染):ghostRef 直指浮卡 DOM,
  // onDragPointerMove 直接写 style.transform 跟手;ghostPosRef 记最新光标
  // 坐标,供浮卡挂载首帧取初值(防 0,0 闪现)。
  const ghostRef = useRef<HTMLDivElement | null>(null);
  const ghostPosRef = useRef({ x: 0, y: 0 });

  // ---- 传输面(Task 13;Task 9 抽屉化)状态 ----
  // 作业快照 Map(事件 reducer 收敛;终态不摘除——jobs 只增是 daemon V1
  // 已知面,抽屉是持久历史面照显示,可见性收敛全在抽屉内)。连接种子 =
  // transfer.list。
  const [jobs, setJobs] = useState<Map<string, JobSnapUI>>(new Map());
  // 控制操作在途的作业 id(pause/resume/cancel/reinitiate/clear 置灰防连点)。
  const [busyJobs, setBusyJobs] = useState<Set<string>>(new Set());
  // 窗内通知回显 toast 队列(event.notify;上限 3,超出丢最旧,5s 自隐)。
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const toastSeq = useRef(0);
  // Shift 范围选择的尾行(Task 13 打磨:范围段以更深底色区分普通选中;
  // 非 Shift 点击即清)。锚点仍是 anchorPath。
  const [rangeEnd, setRangeEnd] = useState<string | null>(null);

  // ---- 更新面(Task 15)状态 ----
  // 更新快照(event.update 经 reducer 收敛;RPC 返回与挂载对账同入收口);
  // ref 镜像供事件处理器读前一帧(reducer 幂等/丢弃判定需要 prev)。
  const [updateSnap, setUpdateSnap] = useState<UpdateSnapshot | null>(null);
  const updateSnapRef = useRef<UpdateSnapshot | null>(null);
  // 更新页覆盖层开合(独立于登录态,不进 nav 栈)。
  const [updatePageOpen, setUpdatePageOpen] = useState(false);
  // check/apply RPC 在途(更新页按钮置灰防连点)。
  const [updateBusy, setUpdateBusy] = useState(false);

  // ---- 登录审批/被移除(V3-1 Task 10)状态 ----
  // 新终端审批请求(event.approval;单槽,后到换前——被顶掉的悬置会话由
  // 服务端 TTL(300s)收口)。受信终端据此弹主窗模态,同意/拒绝 →
  // approval.respond(Bearer=当前会话)。
  const [approvalReq, setApprovalReq] = useState<ApprovalEvent | null>(null);
  // 应答 RPC 在途(弹窗两键置灰防连点)。
  const [approvalBusy, setApprovalBusy] = useState(false);
  // 本设备被移出账号横幅(event.device_evicted):回登录面常显,重新
  // 登录成功(event.login true)即清;主动登出不置位。
  const [evicted, setEvicted] = useState(false);

  const refresh = useCallback(() => {
    void loadAll(setHello, setStatus, setDevices);
  }, []);

  /// 清空浏览期瞬时态:选择、Shift 锚点与搜索词。导航换位/复位/搜索词
  /// 变更时调用(搜索词清空保证状态栏计数始终对应当前渲染列表)。
  const clearTransient = useCallback(() => {
    setSelection(new Set());
    setAnchorPath(null);
    setSearchQ("");
    setRangeEnd(null);
  }, []);

  /// 登出/断连复位:回设备视图、清空历史与浏览状态,并令在途列表响应
  /// 过期(seq 前移)。
  const resetToDevices = useCallback(() => {
    seqRef.current += 1;
    setNav({ back: [], fwd: [], current: { kind: "devices" } });
    setBrowse(BROWSE_IDLE);
    clearTransient();
  }, [clearTransient]);

  /// 传输作业种子(Task 13;Task 9 抽屉化):连接建立时全量拉 transfer.list
  /// **重置**作业面(daemon 可能已换实例,旧实例作业不复存在,残留 Map
  /// 反而显示幽灵行);抽屉是持久历史面,终态/中断行照显示(操作可用性
  /// 矩阵收敛在抽屉,无需隐藏面)。失败静默(事件流随后自会补齐进度;
  /// 种子只影响「起笔可见性」)。reinitiate/clear 成功后的对账重拉共用
  /// 本入口(翻新换 id/记录摘除即刻反映进抽屉)。
  const seedTransfers = useCallback(() => {
    void daemonTransferList()
      .then((snaps) => setJobs(seedJobs(snaps)))
      .catch(() => {});
  }, []);

  /// 窗内 toast 入队(event.notify 回显与传输控制错误共用;**不堆叠**:
  /// 上限 3 条,超出丢最旧)。5s 自隐计时挂在 ToastRow 自身 effect,到期
  /// 经 expireToast 出队。
  const pushToast = useCallback((item: Omit<ToastItem, "id">) => {
    setToasts((prev) => {
      const next = [...prev, { ...item, id: ++toastSeq.current }];
      return next.length > 3 ? next.slice(next.length - 3) : next;
    });
  }, []);

  const expireToast = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  /// 审批应答统一执行面(V3-1):置在途防连点 → RPC → 成功收模态(静默;
  /// 同意后对端轮询领票登录由对端自己驱动);失败保留模态可重试(会话
  /// TTL 内仍可改判),错误经 toast(与其他控制面同口径)。
  const respondApproval = useCallback(
    (allow: boolean) => {
      const req = approvalReq;
      if (req == null || approvalBusy) return;
      setApprovalBusy(true);
      void approvalRespond(req.session_id, allow)
        .then(() => setApprovalReq(null))
        .catch((e: unknown) => {
          pushToast({
            title: t("approve.failed"),
            body: toRpcError(e).message,
            tone: "error",
          });
        })
        .finally(() => setApprovalBusy(false));
    },
    [approvalReq, approvalBusy, pushToast, t],
  );

  /// 控制面错误 → toast(friendlyError 整形,与 notifyError 同口径;出口
  /// 不同:传输卡片/全局面用 toast,目录面沿用提示条)。
  const toastError = useCallback(
    (e: unknown) => {
      const fe = friendlyError(e);
      pushToast({
        title: t("transfer.controlFailed"),
        body: fe.kind === "offline" ? t(fe.text) : fe.text,
        tone: "error",
      });
    },
    [pushToast, t],
  );

  /// 传输控制统一执行面(Task 13;Task 9 增 reinitiate/clear):置在途防
  /// 连点 → RPC → 失败转 toast(remote_copy 拒 pause 的 daemon 业务错误、
  /// clear 对端离线 code 3 即此呈现);成功静默(行面状态由 event.transfer
  /// /种子重拉驱动,控制面不抢戏)。
  const jobControl = useCallback(
    (id: string, run: (id: string) => Promise<void>) => {
      setBusyJobs((prev) => new Set(prev).add(id));
      void run(id)
        .catch(toastError)
        .finally(() => {
          setBusyJobs((prev) => {
            const next = new Set(prev);
            next.delete(id);
            return next;
          });
        });
    },
    [toastError],
  );

  /// 重新发起(Task 9,spec §5.1):失败/中断记录翻新重发——新作业 id 由
  /// daemon 铸造、旧记录摘除,RPC 成功后全量重拉让旧行出列、新行入场。
  const reinitiateJob = useCallback(
    (id: string) =>
      jobControl(id, async (x) => {
        await daemonTransferReinitiate(x);
        seedTransfers();
      }),
    [jobControl, seedTransfers],
  );

  /// 清除(Task 9,spec §5.2「各清各的」):记录摘除(对端残留回收在
  /// daemon 侧收口)后全量重拉;对端离线失败任务保留,错误经 toast。
  const clearJob = useCallback(
    (id: string) =>
      jobControl(id, async (x) => {
        await daemonTransferClear(x);
        seedTransfers();
      }),
    [jobControl, seedTransfers],
  );

  /// 更新帧统一收口(Task 15):事件帧/RPC 返回/挂载对账全走同一 reducer
  /// 入口(ref 镜像读 prev;reducer 幂等,同帧重复返回原引用不重渲染)。
  /// available 翻转(此前非 available)→ pushToast「发现新版本 X」,
  /// 「查看」动作开更新页(toast 不自动出队,5s TTL 照常)。
  const applyUpdateFrame = useCallback(
    (frame: UpdateEvent) => {
      const prev = updateSnapRef.current;
      const next = updateStateFromEvents(prev, frame);
      updateSnapRef.current = next;
      setUpdateSnap(next);
      if (next.state === "available" && prev?.state !== "available") {
        pushToast({
          title: t("update.toastTitle", { version: next.latest ?? "" }),
          body: t("update.toastBody"),
          tone: "info",
          action: { label: t("update.toastAction"), onClick: () => setUpdatePageOpen(true) },
        });
      }
    },
    [pushToast, t],
  );

  /// 更新 RPC 统一执行面(Task 15):置在途防连点 → RPC → 返回的状态
  /// JSON 与事件同形,走同一收口;失败(业务失败已由 daemon 落 failed 态
  /// + 事件广播;安装序列中连接断开是正常收尾)→ 重拉 update.state 对账
  /// 兜底,快照是唯一事实源,RPC 错误串不单独呈现。
  const runUpdateRpc = useCallback(
    (run: () => Promise<UpdateSnapshot>) => {
      setUpdateBusy(true);
      void run()
        .then(applyUpdateFrame)
        .catch(() => {
          void updateState().then(applyUpdateFrame).catch(() => {});
        })
        .finally(() => setUpdateBusy(false));
    },
    [applyUpdateFrame],
  );

  const handleUpdateApply = useCallback(
    (version?: string) => runUpdateRpc(() => updateApply(version)),
    [runUpdateRpc],
  );
  const handleUpdateCheck = useCallback(() => runUpdateRpc(updateCheck), [runUpdateRpc]);

  /// 主题同步(P4b Task 1):data-theme 落 <html>(全组件经 CSS 变量取色)
  /// + localStorage 持久化;窗口标题栏尽力跟随(dark→dark,其余→light),
  /// API 不存在(非 Tauri 上下文)或调用失败时静默吞掉跟随系统,不阻塞。
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    saveTheme(theme);
    try {
      void getCurrentWindow()
        .setTheme(theme === "dark" ? "dark" : "light")
        .catch(() => {});
    } catch {
      // 无窗口对象/旧版 API 不支持:标题栏主题跟随不可用,静默
    }
  }, [theme]);

  /// 冷启动 `--update-page` 路由标记(Task 15):ui.exe 直接带参启动(无
  /// 既有实例转发)时 Rust 侧置位,挂载后拉取消费(取走即清)。
  useEffect(() => {
    let stale = false;
    void takeOpenUpdateRequest()
      .then((req) => {
        if (req && !stale) setUpdatePageOpen(true);
      })
      .catch(() => {});
    return () => {
      stale = true;
    };
  }, []);

  /// 更新页挂载对账(Task 15):拉一次 update.state——UI 启动前已发生、
  /// 无新事件可推的状态翻转(冷启动时 daemon 已检得新版)由此补齐。
  useEffect(() => {
    if (!updatePageOpen) return;
    let stale = false;
    void updateState()
      .then((snap) => {
        if (!stale) applyUpdateFrame(snap);
      })
      .catch(() => {});
    return () => {
      stale = true;
    };
  }, [updatePageOpen, applyUpdateFrame]);

  /// 起作业后的一次性对账(Task 13):event.transfer 事件帧不带 src/dst
  /// (daemon emit_locked 契约),新作业首帧落 Map 时路径面为空——卡片会
  /// 一直显示「… → …」。transfer.list 全量**合并**(非重置,隐藏面不动)
  /// 一次把 src/dst 补齐;start 返回时作业已入注册表,合并必含它。
  const mergeTransfers = useCallback(() => {
    void daemonTransferList()
      .then((snaps) => {
        setJobs((prev) => snaps.reduce((m, s) => applyTransferEvents(m, s), prev));
      })
      .catch(() => {});
  }, []);

  const loggedIn = status?.logged_in ?? hello?.logged_in ?? false;

  // 快捷目录 store(V3-3 Task 8):登录即订阅 data_changed+首拉,状态经
  // 订阅镜像;Sidebar 快捷区与新建对话框经此消费(createFav 等)。声明
  // 在连接/聚焦效应之前(终审修①):重连与 focus 兜底重拉要引用
  // refresh,hook 须先声明(仅读上方 hello/status,前移无环)。
  const favStore = useFavorites(loggedIn);

  useEffect(() => {
    refresh();
    const unlisteners: Array<Promise<() => void>> = [
      // 连接建立(含冷启动拉起后的首次连上):重拉全量 + 回设备视图
      // (daemon 可能已换实例,历史浏览位不再可信)。快捷列表一并重拉
      // (终审修①,spec §3.2 重连全量拉兜底):data_changed 只推在线
      // 端点,信令断线期间他端改快捷,重连后无事件可补,须全量拉;
      // 未登录时 store 内部有 loggedIn 门(refresh 自挡,免外加)。
      listenConnected(() => {
        refresh();
        favStore.refresh();
        resetToDevices();
        seedTransfers();
      }),
      // 信令在线状态:只翻 status.online,避免整包覆盖(其余字段未变)
      listenOnline((online) =>
        setStatus((s) => (s == null ? s : { ...s, online })),
      ),
      // 设备快照:全量替换
      listenDevices((list) => setDevices(list)),
      // 登录态翻转(托盘/CLI 侧登录登出同样到达):全量刷新;登出回引导态;
      // 登录成功清「已被移除」横幅(V3-1:被踢重登的收口点);登出清
      // fav 位 SWR 缓存(§16.4:换账号旧账号目录内容绝不可瞬时复用)
      listenLogin((loggedIn) => {
        refresh();
        if (loggedIn) setEvicted(false);
        else {
          clearFavCache();
          resetToDevices();
        }
      }),
      // 新终端审批请求(V3-1):受信终端弹主窗模态(单槽,后到换前)
      listenApproval((ev) => setApprovalReq(ev)),
      // 被移出账号(V3-1):daemon 已清会话并随发 event.login false;此
      // 处置横幅 + 全量刷新回登录面;在途审批弹窗一并收(应答必失败);
      // fav 位 SWR 缓存一并清(强制登出同 §16.4 清场纪律)
      listenDeviceEvicted(() => {
        setEvicted(true);
        setApprovalReq(null);
        clearFavCache();
        refresh();
      }),
      // 传输作业事件(Task 13):reducer 收敛(乱序/重复/终态粘滞全在
      // lib/transfer 收口),卡片与状态栏速率随之渲染。
      listenTransfer((evt) => setJobs((prev) => applyTransferEvents(prev, evt))),
      // 接收完成通知(Task 13):daemon 已自发 OS 系统通知,此处仅窗内
      // toast 回显同文案(5s 自隐,队列上限 3)。
      listenNotify((n) => pushToast({ title: n.title, body: n.body, tone: "info" })),
      // 更新状态机事件(Task 15):reducer 收敛(乱序旧帧/终态粘滞/failed
      // 保错全在 lib/update 收口),available 翻转由收口面 toast。
      listenUpdate((frame) => applyUpdateFrame(frame)),
      // 更新页打开路由(Task 15):托盘「检查更新…」拉起 ui.exe
      // --update-page → 单实例回调转发本事件 → 开更新页覆盖层。
      listenOpenUpdate(() => setUpdatePageOpen(true)),
    ];
    return () => {
      void Promise.all(unlisteners).then((uns) => uns.forEach((u) => u()));
    };
  }, [refresh, favStore.refresh, resetToDevices, seedTransfers, pushToast, applyUpdateFrame]);

  /// 窗口 focus 重拉 devices(Task 13 打磨):切走再切回时设备在线态/新
  /// 设备尽快对齐(事件只推增量翻转,focus 全量重拉是对账面);失败静默
  /// (事件流兜底)。hello/status 不重拉:登录/在线态均有事件翻转面。
  /// 快捷列表一并重拉(终审修①,与 devices 同理的对账面;未登录时
  /// store 内部门挡,零开销)。
  useEffect(() => {
    const onFocus = () => {
      favStore.refresh();
      void daemonDevices()
        .then((d) => setDevices(d.devices))
        .catch(() => {});
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [favStore.refresh]);

  const selfDeviceId = hello?.device?.device_id ?? status?.device?.device_id ?? null;
  const [favNameOpen, setFavNameOpen] = useState(false);
  const [favBusy, setFavBusy] = useState(false);
  // 重命名目标 id(V3-3 Task 13):null = 命名对话框处于新建态;非空 =
  // 重命名态(FavNameDialog initial 预填 + 提交走 renameFav)。取消/成功/
  // 目标被删(他端删除随 data_changed 同步)三路清空。
  const [favRenameId, setFavRenameId] = useState<string | null>(null);
  // 侧栏快捷行右键菜单打开态(V3-3 Task 13):目标 id + 触发点视口坐标
  // (null = 收起;项集渲染期按 menuFav 从 store 现值装配)。
  const [favMenu, setFavMenu] = useState<{ id: string; x: number; y: number } | null>(null);
  // 管理屏目标 id(V3-3 Task 13,自 T8 暂存位点亮):manageFav 从 store
  // 现值解析,他端删除快捷后解析为 null,弹层卸载且占线自动解除
  // (memberPickFav 同款语义)。
  const [manageFavId, setManageFavId] = useState<string | null>(null);
  // 「删除快捷目录」确认梯目标 id(V3-3 Task 13;null = 关闭)。删除=只
  // 解除链接,不触碰远端文件(spec §3.1)。
  const [favDelFor, setFavDelFor] = useState<string | null>(null);
  // 快捷视图行数(FavView 上抛;状态栏 fav 位 itemCount 消费)。
  const [favRowCount, setFavRowCount] = useState(0);
  // 快捷位渲染行(T11 动作对等):FavView onRowsChange 上抛(渐进扇出逐帧
  // 更新),ref 持有不触发渲染;下载/删除/复制剪切处理器调用时读现值做
  // 跨源分组(favCtx 选中集映射同源)。
  const favRowsRef = useRef<MixedRow[]>([]);
  const handleFavRowsChange = useCallback((rs: MixedRow[]) => {
    favRowsRef.current = rs;
  }, []);
  // 「添加到快捷目录」picker 挂起批(V3-3 Task 11,目录位):候选目录的
  // 设备与路径集(null = picker 关闭;开梯即定死,防导航串台)。
  const [favPickFor, setFavPickFor] = useState<{ device: string; paths: string[] } | null>(null);
  // picker 内「新建快捷目录」流(T11 Final ruling):命名对话框提交成功后,
  // 按 id 差集认领新条目 → 直接 addMembers → toast → 双关(promise 链内
  // 读不到新渲染的 store 快照,认领收口在效应读渲染态)。
  const [favPickNew, setFavPickNew] = useState<{
    prevIds: ReadonlySet<string>;
    device: string;
    paths: string[];
  } | null>(null);
  // 快捷位多组删除确认梯(T11):need_force(回收站不可用)组快照。dir 位
  // confirmDel 形态零改,独立实例 + 独立弹窗(最小侵入);确认后逐组 force
  // 重发(设备取开梯快照,与导航解耦——同 confirmDel 注释的 MUST-FIX)。
  const [favDel, setFavDel] = useState<{
    groups: { device: string; paths: string[] }[];
  } | null>(null);
  // 「存入哪个成员」选择层挂起批(V3-3 T12 拖放三路):多成员快捷的
  // 拖放(favMemberDropFor,携带源组集)与粘贴(favMemberPasteFor)各自
  // 暂存,共用一个 FavMemberPicker 实例;onPick 后按各自管线落目标。
  // null = 关闭。fav 本体渲染期从 store 解析(memberPickFav),他端删除
  // 快捷后解析为 null,弹层随之消失且占线自动解除(无需额外清理效应)。
  const [favMemberDropFor, setFavMemberDropFor] = useState<{
    favId: string;
    sources: DragSourceSel[];
  } | null>(null);
  const [favMemberPasteFor, setFavMemberPasteFor] = useState<{ favId: string } | null>(null);
  // 快捷位解析(T9):当前导航 fav id → store 清单条目;清单已无此 id
  // (他端删除随 data_changed 同步)→ gone 提示面(渲染分支处理)。
  // id 先提为 const(回调内属性收窄失效,TS 纪律同 dirKey 提取)。
  const currentFavId = nav.current.kind === "fav" ? nav.current.id : null;
  const currentFav =
    currentFavId != null
      ? (favStore.state.doc.favorites.find((f) => f.id === currentFavId) ?? null)
      : null;
  // 成员选择层的快捷本体(T12):拖放/粘贴两挂起批共用一个解析面——
  // 从 store 现值解析,清单已无此 id(他端删除随 data_changed 同步)→
  // null,弹层卸载且 modalBusy 占线自动解除。
  const memberPickFavId = favMemberDropFor?.favId ?? favMemberPasteFor?.favId ?? null;
  const memberPickFav =
    memberPickFavId != null
      ? (favStore.state.doc.favorites.find((f) => f.id === memberPickFavId) ?? null)
      : null;
  // 快捷管理三目标本体(V3-3 Task 13):右键菜单/管理屏/删除确认都从
  // store 现值解析,清单已无此 id(他端删除随 data_changed 同步)→ null
  // ——菜单空清单防御不渲染、管理屏条件卸载(占线自动解除)、删除确认
  // 文案名兜底空串。
  const menuFav =
    favMenu != null
      ? (favStore.state.doc.favorites.find((f) => f.id === favMenu.id) ?? null)
      : null;
  const manageFav =
    manageFavId != null
      ? (favStore.state.doc.favorites.find((f) => f.id === manageFavId) ?? null)
      : null;
  const favDelTarget =
    favDelFor != null
      ? (favStore.state.doc.favorites.find((f) => f.id === favDelFor) ?? null)
      : null;
  // 重命名目标本体(T13):FavNameDialog initial 预填与提交分派读;被删 →
  // null(下方效应双清收口)。
  const renameTarget =
    favRenameId != null
      ? (favStore.state.doc.favorites.find((f) => f.id === favRenameId) ?? null)
      : null;

  /// 重命名目标被删(他端删除随 data_changed 同步)→ 双清收口:命名对话
  /// 框关闭并退出重命名态,防悬空 id 停留(memberPickFav 解析语义同款)。
  useEffect(() => {
    if (favRenameId != null && renameTarget == null) {
      setFavNameOpen(false);
      setFavRenameId(null);
    }
  }, [favRenameId, renameTarget]);

  /// 目录列表拉取:当前 Loc 为目录位或快捷命名空间位时发起(favdir 复用
  /// 同一管线,返工 §13.6:取数键=device\0joinRel(base, rel) 原生路径);
  /// seqRef 单调递增 + cleanup 置 stale,双保险丢弃过期响应(快速连续导航
  /// 不串台)。
  /// favdir 当前原生路径(joinRel 拼段,nav 纪律;非 favdir 位为 null):
  /// 取数键/拖放落点键/粘贴目标/复制路径共用同一份。
  const favdirPath =
    nav.current.kind === "favdir"
      ? joinRel(nav.current.base, nav.current.rel)
      : null;
  const dirKey =
    nav.current.kind === "dir"
      ? `${nav.current.device}\u0000${nav.current.path}`
      : nav.current.kind === "favdir"
        ? `${nav.current.device}\u0000${favdirPath}`
        : null;
  useEffect(() => {
    if (dirKey == null) return;
    const nul = dirKey.indexOf("\u0000");
    const device = dirKey.slice(0, nul);
    const path = dirKey.slice(nul + 1);
    const seq = ++seqRef.current;
    setBrowse({ loading: true, listing: null, error: null });
    let stale = false;
    daemonList(device, path)
      .then((l) => {
        if (stale) return;
        const out = applyListing(seq, seqRef.current, {
          entries: l.entries,
          truncated: l.truncated,
        });
        if (out == null) return; // 过期响应:丢弃
        setBrowse({ loading: false, listing: out, error: null });
      })
      .catch((e: unknown) => {
        if (stale || seq < seqRef.current) return;
        setBrowse({ loading: false, listing: null, error: toRpcError(e).message });
      });
    return () => {
      stale = true;
    };
  }, [dirKey, reloadTick]);

  /// 导航:压栈前进(重复位亦入栈,历史保真)+ 清空选择/锚点/搜索。
  const navigate = useCallback(
    (next: Loc) => {
      setNav((h) => navGo(h, next));
      clearTransient();
    },
    [clearTransient],
  );

  const goBack = useCallback(() => {
    setNav((h) => navBack(h) ?? h);
    clearTransient();
  }, [clearTransient]);

  const goForward = useCallback(() => {
    setNav((h) => navForward(h) ?? h);
    clearTransient();
  }, [clearTransient]);

  /// 上级:目录位逐级上溯,到根后即设备层;已在设备层则不动。
  const goUp = useCallback(() => {
    setNav((h) => {
      const up = parentLoc(h.current);
      return sameLoc(up, h.current) ? h : navGo(h, up);
    });
    clearTransient();
  }, [clearTransient]);

  const deviceName = useCallback(
    (deviceId: string) => devices.find((d) => d.device_id === deviceId)?.name ?? deviceId,
    [devices],
  );

  const deviceOnline = useCallback(
    (deviceId: string) =>
      devices.find((d) => d.device_id === deviceId)?.online ?? false,
    [devices],
  );

  /// 地址栏编辑态提交(P4b Task 4;§15.5 扩快捷命名空间):fav/favdir →
  /// navigate(快捷混合视图 / 命名空间位);nav/deviceRoot → navigate(入
  /// 导航历史,解析规则全在 lib/address.ts 纯函数);error → toast 文案
  /// (deviceNotFound 携候选名插值;favAmbiguous = 多成员快捷无法按地址
  /// 定成员;诸键均阻断跳转)。空输入已在 AddressBar 按取消处理,不会以
  /// error 到达此处。
  const handleAddressSubmit = useCallback(
    (target: AddressTarget) => {
      switch (target.kind) {
        case "fav":
          navigate({ kind: "fav", id: target.favId });
          break;
        case "favdir":
          navigate({
            kind: "favdir",
            favId: target.favId,
            device: target.device,
            base: target.base,
            rel: target.rel,
          });
          break;
        case "nav":
          navigate({ kind: "dir", device: target.device, path: target.path });
          break;
        case "deviceRoot":
          navigate({ kind: "dir", device: target.device, path: "" });
          break;
        case "error": {
          const title =
            target.code === "deviceNotFound"
              ? t("address.deviceNotFound", { name: target.name ?? "" })
              : target.code === "deviceAmbiguous"
                ? t("address.deviceAmbiguous")
                : target.code === "favAmbiguous"
                  ? t("address.favAmbiguous")
                  : t("address.needDeviceOrAbsolute");
          pushToast({ title, body: "", tone: "error" });
          break;
        }
      }
    },
    [navigate, pushToast, t],
  );

  /// 复制路径(P4b Task 4):当前目录**原生完整路径** → daemonCopyText
  /// (navigator.clipboard 优先、桥 copy_text 兜底)→ 轻 toast「路径已
  /// 复制」;失败 toast 原报文。入口按钮已在设备层/卷列表位禁用,此为
  /// 防御(卷列表位无原生路径可复制)。favdir 位(返工 §13.6 Review
  /// Focus #2 裁定)给成员原始绝对路径 base+rel(joinRel)——命名空间
  /// 不破既有功能。
  const handleCopyPath = useCallback(() => {
    const cur = nav.current;
    const path =
      cur.kind === "dir"
        ? cur.path === ""
          ? null
          : cur.path
        : cur.kind === "favdir"
          ? joinRel(cur.base, cur.rel)
          : null;
    if (path == null) return;
    void daemonCopyText(path)
      .then(() => pushToast({ title: t("address.pathCopied"), body: "", tone: "info" }))
      .catch((e: unknown) => {
        pushToast({ title: t("address.copyPath"), body: String(e), tone: "error" });
      });
  }, [nav.current, pushToast, t]);

  /// 收件箱导航(V3-3 Task 8 建;返工裁定五 §17.1 起为着陆页收件箱行
  /// 入口):本机收件目录位;selfDeviceId 或 recv_dir 缺失 → no-op(着陆
  /// 页 inboxAvailable 门与导航门控各自独立)。
  const openInbox = useCallback(() => {
    if (selfDeviceId != null && status?.recv_dir) {
      navigate({ kind: "dir", device: selfDeviceId, path: status.recv_dir });
    }
  }, [selfDeviceId, status, navigate]);

  /// 快捷编辑成功后的视图对账(T9 遗留收口):正在浏览该快捷(混合视图
  /// 按成员渐进扇出,成员集变化后行面需重种子)→ reloadTick +1 硬重拉。
  /// 成员移除(管理屏)与 picker 内添加两路共用。
  const bumpIfViewing = useCallback(
    (id: string) => {
      if (nav.current.kind === "fav" && nav.current.id === id) {
        setReloadTick((x) => x + 1);
      }
    },
    [nav.current],
  );

  /// 新建快捷提交(V3-3 Task 8;T11 扩 picker 内新建流):busy 防连点 →
  /// createFav(store 内 kv.get 取最新→改→kv.put 整单);成功 toast「已创
  /// 建」+关对话框,失败 toast 错误、对话框保持可改可重试(与 respondApproval
  /// 同口径;store 返回 false,错误明细已被 store 吞掉,不逐字透传)。
  /// picker 内新建(favPickFor 非空,T11 Final ruling):提交成功后不直接
  /// 收口——记清单 id 快照挂 favPickNew,由效应认领新条目直加成员并双关
  /// (此流不弹「已创建」,紧随的「已加入」即结果)。
  const handleFavCreate = useCallback(
    (name: string) => {
      if (favBusy) return;
      setFavBusy(true);
      const fromPicker = favPickFor != null;
      const prevIds = fromPicker
        ? new Set(favStore.state.doc.favorites.map((f) => f.id))
        : null;
      void favStore
        .createFav(name)
        .then((ok) => {
          if (ok) {
            setFavNameOpen(false);
            if (fromPicker && prevIds != null && favPickFor != null) {
              setFavPickNew({ prevIds, device: favPickFor.device, paths: favPickFor.paths });
            } else {
              pushToast({ title: t("favorites.created"), body: "", tone: "info" });
            }
          } else {
            pushToast({ title: t("favorites.createFailed"), body: "", tone: "error" });
          }
        })
        .finally(() => setFavBusy(false));
    },
    [favBusy, favStore, favPickFor, pushToast, t],
  );

  /// picker 内新建的直加收口(T11 Final ruling):新建落库(put 成功才
  /// setState,订阅镜像进渲染态)后按 id 差集认领新条目——差集取**尾**
  /// (createFavorite 恒尾部追加,尾项必为本端新建),直接 addMembers →
  /// toast「已加入」→ 双关(命名对话框 + picker)。全为重复(他端并发已
  /// 加同名成员,理论边)→「已在其中」;写入失败(离线/只读守卫)→ 错误
  /// toast,双层照关(快捷已建成,可从侧栏重试添加)。
  useEffect(() => {
    if (favPickNew == null) return;
    let found: Favorite | null = null;
    for (const f of favStore.state.doc.favorites) {
      if (!favPickNew.prevIds.has(f.id)) found = f;
    }
    if (found == null) return; // 尚未落库:等下一帧
    const fresh = found; // 提 const:回调内属性收窄失效(TS 纪律同 currentFavId)
    const spec = favPickNew;
    setFavPickNew(null);
    setFavNameOpen(false);
    setFavPickFor(null);
    setFavBusy(true);
    void favStore
      .addMembers(fresh.id, spec.paths.map((p) => ({ device: spec.device, path: p })))
      .then((added) => {
        pushToast({
          title:
            added > 0
              ? t("favorites.addedTo", { name: fresh.name })
              : t("favorites.alreadyIn"),
          body: "",
          tone: "info",
        });
        if (added > 0) bumpIfViewing(fresh.id);
      })
      .catch(() => {
        pushToast({ title: t("favorites.addFailed"), body: "", tone: "error" });
      })
      .finally(() => setFavBusy(false));
  }, [favPickNew, favStore, pushToast, t, bumpIfViewing]);

  /// picker 点选已有快捷(T11):busy 防连点 → 目标内按 memberKey 预去重
  /// (全重复直接「已在其中」收口,不发 store 回写);addMembers 实际新增
  /// >0 →「已加入 {名}」;=0(store 写入失败/只读守卫,store 吞错不区分,
  /// 预去重后已排除纯重复)→ 错误 toast。收口即关 picker。
  const handleFavPick = useCallback(
    (id: string) => {
      if (favBusy || favPickFor == null) return;
      const target = favStore.state.doc.favorites.find((f) => f.id === id);
      if (target == null) return; // 清单已变(他端删除随 data_changed 同步)
      const spec = favPickFor;
      const existing = new Set(target.members.map((m) => memberKey(m)));
      const fresh = spec.paths.filter(
        (p) => !existing.has(memberKey({ device: spec.device, path: p })),
      );
      if (fresh.length === 0) {
        setFavPickFor(null);
        pushToast({ title: t("favorites.alreadyIn"), body: "", tone: "info" });
        return;
      }
      setFavBusy(true);
      void favStore
        .addMembers(id, fresh.map((p) => ({ device: spec.device, path: p })))
        .then((added) => {
          setFavPickFor(null);
          pushToast({
            title: added > 0 ? t("favorites.addedTo", { name: target.name }) : t("favorites.addFailed"),
            body: "",
            tone: added > 0 ? "info" : "error",
          });
          if (added > 0) bumpIfViewing(id);
        })
        .catch(() => {
          pushToast({ title: t("favorites.addFailed"), body: "", tone: "error" });
        })
        .finally(() => setFavBusy(false));
    },
    [favBusy, favPickFor, favStore, pushToast, t, bumpIfViewing],
  );

  /// ---- 快捷管理面(V3-3 Task 13)----

  /// 侧栏快捷行右键(T13):记目标 id + 触发点坐标开 TreeNodeMenu(项集
  /// 渲染期按 menuFav 装配;菜单关闭由 TreeNodeMenu 回调收口)。
  const handleFavContextMenu = useCallback((id: string, x: number, y: number) => {
    setFavMenu({ id, x, y });
  }, []);

  /// 侧栏快捷行右键菜单项(T13):目标快捷存在才装配(他端删除后 menuFav
  /// 为 null → 空清单,TreeNodeMenu 防御不渲染);删除项 danger 红字。
  /// 动作:打开=钻取 fav Loc;重命名=命名对话框切重命名态;管理成员=开
  /// 管理屏;删除快捷=挂「只解除链接」确认梯。
  const favMenuItems: TreeNodeMenuItem[] =
    menuFav == null
      ? []
      : [
          {
            key: "open",
            label: t("favorites.menuOpen"),
            onPick: () => navigate({ kind: "fav", id: menuFav.id }),
          },
          {
            key: "rename",
            label: t("favorites.menuRename"),
            onPick: () => {
              setFavRenameId(menuFav.id);
              setFavNameOpen(true);
            },
          },
          { key: "manage", label: t("favorites.menuManage"), onPick: () => setManageFavId(menuFav.id) },
          {
            key: "delete",
            label: t("favorites.menuDelete"),
            danger: true,
            onPick: () => setFavDelFor(menuFav.id),
          },
        ];

  /// 管理屏成员移除(T13):busy 防连点 → store removeMember → 成功 toast
  /// + bumpIfViewing(正在浏览该快捷则硬重种子);失败错误 toast、弹层保
  /// 持(fav 本体随 props 重投影,put 成功后行即消失)。
  const handleFavRemoveMember = useCallback(
    (member: FavMember) => {
      if (favBusy || manageFav == null) return;
      const id = manageFav.id;
      setFavBusy(true);
      void favStore
        .removeMember(id, member)
        .then((ok) => {
          if (ok) {
            pushToast({ title: t("favorites.memberRemoved"), body: "", tone: "info" });
            bumpIfViewing(id);
          } else {
            pushToast({ title: t("favorites.memberRemoveFailed"), body: "", tone: "error" });
          }
        })
        .finally(() => setFavBusy(false));
    },
    [favBusy, manageFav, favStore, pushToast, t, bumpIfViewing],
  );

  /// 重命名提交(T13):busy 防连点 → store renameFav → 成功关对话框 +
  /// toast(名随 store 下发,侧栏/视图零额外刷新);失败错误 toast 可重
  /// 试(与 handleFavCreate 同口径)。
  const handleFavRename = useCallback(
    (name: string) => {
      if (favBusy || renameTarget == null) return;
      const id = renameTarget.id;
      setFavBusy(true);
      void favStore
        .renameFav(id, name)
        .then((ok) => {
          if (ok) {
            setFavNameOpen(false);
            setFavRenameId(null);
            pushToast({ title: t("favorites.renamed"), body: "", tone: "info" });
          } else {
            pushToast({ title: t("favorites.renameFailed"), body: "", tone: "error" });
          }
        })
        .finally(() => setFavBusy(false));
    },
    [favBusy, renameTarget, favStore, pushToast, t],
  );

  /// 「删除快捷目录」确认终步(T13):store deleteFav(只解除链接,绝不
  /// 触碰远端文件)→ 成功 toast 已删除;正浏览该快捷 → 回设备视图;管理
  /// 屏恰开着该快捷 → 清暂存(fav 本体解析为 null 已会卸载,双保险)。
  /// store 在途编辑门同步查(state.busy 为引擎现值,非 React 镜像),防
  /// 确认双击的第二发把 mutate 重入拒断误报成「删除失败」。
  const confirmFavDelete = useCallback(() => {
    const id = favDelFor;
    setFavDelFor(null);
    if (id == null || favStore.state.busy) return;
    setFavBusy(true);
    void favStore
      .deleteFav(id)
      .then((ok) => {
        if (ok) {
          pushToast({ title: t("favorites.deleted"), body: "", tone: "info" });
          // 正浏览该快捷(混合视图或其快捷命名空间位,返工 §13.6)→ 回设备
          // 视图(命名空间的 favId 已悬空,不滞留)。
          const viewing =
            (nav.current.kind === "fav" && nav.current.id === id) ||
            (nav.current.kind === "favdir" && nav.current.favId === id);
          if (viewing) {
            navigate({ kind: "devices" });
          }
          setManageFavId((cur) => (cur === id ? null : cur));
        } else {
          pushToast({ title: t("favorites.deleteFailed"), body: "", tone: "error" });
        }
      })
      .finally(() => setFavBusy(false));
  }, [favDelFor, favStore, pushToast, t, nav.current, navigate]);

  /// 当前浏览设备是否离线(以 devices 快照派生;树/主区门控同源)。
  /// 目录视图与快捷命名空间位专用:设备层/快捷位恒为 false。
  const browsingOffline =
    (nav.current.kind === "dir" || nav.current.kind === "favdir") &&
    !deviceOnline(nav.current.device);

  /// 双击目录行进入子目录(目标离线或正在浏览离线设备即拒绝,门控与树
  /// 一致;离线期双击为 no-op,顶部提示条已是可见解释)。favdir 位双击
  /// (返工 §13.6)= 命名空间内钻取:child 绝对路径反推 rel(relUnderBase;
  /// 异常数据不在 base 之下 → §17.3 收口:留在原位 + toast,不跳普通
  /// dir 位,与 openFavDir/openFavDirMember 同口径)。
  const openDir = useCallback(
    (device: string, path: string) => {
      if (!canEnterDir(deviceOnline(device), browsingOffline)) return;
      const cur = nav.current;
      if (cur.kind === "favdir") {
        const rel = relUnderBase(cur.base, path);
        if (rel != null) {
          navigate({ kind: "favdir", favId: cur.favId, device: cur.device, base: cur.base, rel });
        } else {
          pushToast({ title: t("favorites.navNotReady"), body: "", tone: "error" });
        }
        return;
      }
      navigate({ kind: "dir", device, path });
    },
    [deviceOnline, navigate, browsingOffline, nav.current, pushToast, t],
  );

  /// 快捷位钻取(V3-3 Task 11 管线;返工 §13.6 改 favdir 命名空间):
  /// 菜单「打开」与 FavView 双击同一入口。base/rel 经 favdirLocAt 归一
  /// (最深包含成员为 base,rel=相对段);归一失败(清单未就绪/快捷已
  /// 悬空,§17.3 收口)**留在原位 + toast**,绝不回退普通 dir 位——面包
  /// 屑翻转「Synshu 设备 ▸ …」+ 侧栏高亮跳设备树即返工波 4 用户所见漂移。
  /// 目标离线拒绝(browsingOffline 在 fav 位恒 false,无「正在浏览离线
  /// 设备」语境,与 dir 位 openDir 分开)。
  const openFavDir = useCallback(
    (device: string, path: string) => {
      if (nav.current.kind !== "fav") return;
      const favId = nav.current.id;
      if (!canEnterDir(deviceOnline(device), false)) return;
      const loc = favdirLocAt(favStore.state.doc.favorites, favId, device, path);
      if (loc == null) {
        pushToast({ title: t("favorites.navNotReady"), body: "", tone: "error" });
        return;
      }
      navigate(loc);
    },
    [nav.current, favStore.state.doc.favorites, deviceOnline, navigate, pushToast, t],
  );

  /// 侧栏成员目录节点钻取(V3-3 返工 §13.1/§13.6;R6 修复轮归一):
  /// 成员节点与懒加载子目录节点同一入口,base/rel 经 favdirLocAt 归一——
  /// 任何深度点击都落 {base:成员路径, rel:相对段} 规范形态(面包屑层级
  /// 与上一级链同主区钻入一致;离线节点灰显禁点,门控在树侧);归一失败
  /// (store 快照未就绪/快捷已悬空,§17.3 收口)留在原位 + toast,绝不
  /// 回退普通 dir 位(§17.3:树成员加载慢时导航跳设备位的漂移面收口)。
  const openFavDirMember = useCallback(
    (favId: string, device: string, clickedPath: string) => {
      const loc = favdirLocAt(favStore.state.doc.favorites, favId, device, clickedPath);
      if (loc == null) {
        pushToast({ title: t("favorites.navNotReady"), body: "", tone: "error" });
        return;
      }
      navigate(loc);
    },
    [favStore.state.doc.favorites, navigate, pushToast, t],
  );

  /// 双击设备行进入该设备根(离线行禁入)。
  const enterDevice = useCallback(
    (d: DeviceInfo) => {
      if (!d.online) return;
      navigate({ kind: "dir", device: d.device_id, path: "" });
    },
    [navigate],
  );

  /// 当前渲染行:filterEntries(搜索)后再 sortEntries(排序,目录恒在前)。
  /// 三视图、状态栏计数与 Shift 范围共用同一份,保证所见即所选。
  const rows = useMemo(
    () => (browse.listing == null ? [] : sortEntries(filterEntries(browse.listing.entries, searchQ), sortState, zhCmp())),
    [browse.listing, searchQ, sortState],
  );
  const orderedPaths = useMemo(() => rows.map((r) => r.path), [rows]);

  /// Shift 范围段(Task 13 打磨):锚点↔范围尾行的当前渲染序闭区间。命中
  /// 且被选中的行以 shift-range 底色呈现,与普通选中(单击/Ctrl)区分。
  const rangeSet = useMemo(() => {
    if (anchorPath == null || rangeEnd == null) return new Set<string>();
    const a = orderedPaths.indexOf(anchorPath);
    const b = orderedPaths.indexOf(rangeEnd);
    if (a < 0 || b < 0) return new Set<string>();
    return new Set(orderedPaths.slice(Math.min(a, b), Math.max(a, b) + 1));
  }, [anchorPath, rangeEnd, orderedPaths]);

  /// 行点击:修饰键派生模式后交 applyClickSelect 解析(选择集与锚点)。
  /// Shift 点击另记范围尾行(Task 13 打磨:范围段以更深底色与普通选中
  /// 区分);其余模式清尾行(范围段语义只属于 Shift 扩展)。
  const handleRowClick = useCallback(
    (path: string, e: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }) => {
      const res = applyClickSelect(selection, orderedPaths, path, clickMode(e), anchorPath);
      setSelection(res.selection);
      setAnchorPath(res.anchor);
      setRangeEnd(clickMode(e) === "shift" ? path : null);
    },
    [selection, orderedPaths, anchorPath],
  );

  /// 空白处点击清空选择(锚点一并清除,避免陈旧范围)。框选拖拽后的收尾
  /// click(落点仍是空白)不在此清空——否则会抹掉框选结果;吞一次后复位。
  const handleClearSelect = useCallback(() => {
    if (bandedRef.current) {
      bandedRef.current = false;
      return;
    }
    setSelection(new Set());
    setAnchorPath(null);
  }, []);

  /// 搜索词变更:换词即新视图上下文,选择与锚点清空(状态栏计数与渲染
  /// 列表保持一致)。
  const handleSearch = useCallback(
    (q: string) => {
      setSearchQ(q);
      setSelection(new Set());
      setAnchorPath(null);
    },
    [],
  );

  // ---------- 操作面(Task 11):右键菜单 / 框选 / 删除 ----------

  /// 三视图行测量:宿主内所有 data-row-path 元素的包围盒(client 坐标)。
  /// 每次框选 mousemove 现测(不缓存)——视图切换/滚动/重排后仍与所见一致。
  const measureRowRects = useCallback((): RowRect[] => {
    const host = viewHostRef.current;
    if (host == null) return [];
    const out: RowRect[] = [];
    host.querySelectorAll<HTMLElement>("[data-row-path]").forEach((el) => {
      const p = el.dataset.rowPath;
      if (p != null) out.push({ path: p, rect: el.getBoundingClientRect() });
    });
    return out;
  }, []);

  /// 框选启动(空白处左键 mousedown):清空现选择与 Shift 锚点(与
  /// Ctrl/Shift 互斥约定),挂 document 级 move/up 监听;移动越过 3px 阈值
  /// 才视为拖拽(否则留给 click 清空路径),越阈后每帧按 rubberBandSelect
  /// 解析选择(此时 prev 恒为空集,表现为纯替换)。
  const startBand = useCallback(
    (x0: number, y0: number) => {
      setSelection(new Set());
      setAnchorPath(null);
      bandedRef.current = false;
      const onMove = (ev: MouseEvent) => {
        const dx = ev.clientX - x0;
        const dy = ev.clientY - y0;
        if (!bandedRef.current && Math.hypot(dx, dy) < 3) return;
        bandedRef.current = true;
        setBandRect({
          left: Math.min(x0, ev.clientX),
          top: Math.min(y0, ev.clientY),
          width: Math.abs(dx),
          height: Math.abs(dy),
        });
        const rects = measureRowRects();
        setSelection((prev) =>
          rubberBandSelect({ x: x0, y: y0 }, { x: ev.clientX, y: ev.clientY }, rects, prev),
        );
      };
      const onUp = () => {
        setBandRect(null);
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
      };
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
    },
    [measureRowRects],
  );

  /// 视图宿主 mousedown:仅空白处(非行元素)+ 无修饰键左键启动框选;
  /// 行上按下交给行点击选择逻辑。
  const onViewMouseDown = useCallback(
    (e: React.MouseEvent) => {
      if (e.button !== 0 || e.shiftKey || e.ctrlKey || e.metaKey) return;
      if ((e.target as Element).closest("[data-row-path]") != null) return;
      startBand(e.clientX, e.clientY);
    },
    [startBand],
  );

  /// 行右键:未选中的行先单选(锚点随行,资源管理器同款),再开条目菜单。
  const handleRowContextMenu = useCallback(
    (entry: FsEntry, x: number, y: number) => {
      if (!selection.has(entry.path)) {
        setSelection(new Set([entry.path]));
        setAnchorPath(entry.path);
      }
      setCtxMenu({ x, y, target: entry });
    },
    [selection],
  );

  /// 空白右键:清空选择与锚点后开空白菜单(粘贴占位/刷新)。
  const handleBlankContextMenu = useCallback((x: number, y: number) => {
    setSelection(new Set());
    setAnchorPath(null);
    setCtxMenu({ x, y, target: null });
  }, []);

  /// fav 位行右键(T11 装配):未选中则单选该行(复合键),再以触发时
  /// 选中集映射装配条目菜单(favCtx 标记;菜单项条件复用 dir 位 target
  /// 面,动作处理器按 favRowsRef 现值跨源分组)。行选择走 onSelected 同
  /// 槽 selection(FavView 内部锚点由其点击管线自管,此处不动)。
  const handleFavRowContextMenu = useCallback(
    (row: MixedRow, x: number, y: number) => {
      let selRows: MixedRow[];
      if (selection.has(row.key)) {
        const byKey = new Map(favRowsRef.current.map((r) => [r.key, r] as const));
        selRows = [...selection].flatMap((k) => {
          const r = byKey.get(k);
          return r == null ? [] : [r];
        });
      } else {
        selRows = [row];
        setSelection(new Set([row.key]));
      }
      setCtxMenu({ x, y, target: row.entry, favCtx: { rows: selRows } });
    },
    [selection],
  );

  /// 删除执行体:force=false 回收优先(可回收静默,spec §4.3);批级
  /// 「回收站不可用」→ 挂起永久删除确认(force 整批重发);code 3 →
  /// 「设备离线」提示;其余失败 → 瞬时提示。任何形状的成功后清选择并
  /// 刷新列表(部分失败如实呈现,已成功项生效)。device 由调用方显式
  /// 传入(不在此处读 nav.current——确认梯重发必须用开梯时的设备快照,
  /// 见 confirmDel 注释)。
  const deletePaths = useCallback(
    async (device: string, paths: string[], force: boolean): Promise<void> => {
      try {
        const r = await daemonDelete(device, paths, force);
        if (r.failed.length > 0) {
          setNotice({ text: t("files.deleteFailed", { message: r.failed[0].message }), tone: "error" });
        }
        setSelection(new Set());
        setAnchorPath(null);
        setReloadTick((x) => x + 1);
      } catch (err) {
        const fe = friendlyError(err);
        if (fe.kind === "need_force") {
          // 开梯即定死设备快照(防导航换设备后 force 误删,见 confirmDel 注释)。
          setConfirmDel({ device, paths });
        } else if (fe.kind === "offline") {
          setNotice({ text: t(fe.text), tone: "error" });
        } else {
          setNotice({ text: fe.text, tone: "error" });
        }
      }
    },
    [t],
  );

  /// 删除入口(工具栏/右键菜单/Del 键共用):非空选择 + 在途防重入。
  /// favdir 位(返工 §13.6)同款 as-dir(设备=成员设备,行=path 粒度)。
  const requestDelete = useCallback(
    (paths: string[]) => {
      const cur = nav.current;
      if (deletingRef.current || paths.length === 0) return;
      if (cur.kind !== "dir" && cur.kind !== "favdir") return;
      const device = cur.device;
      deletingRef.current = true;
      void deletePaths(device, paths, false).finally(() => {
        deletingRef.current = false;
      });
    },
    [deletePaths, nav.current],
  );

  /// 确认梯终步:force=true 永久删除整批(与 CLI rm 确认梯同口径)。设备
  /// 取开梯快照(confirmDel.device),不读当前导航。
  const confirmForceDelete = useCallback(() => {
    const device = confirmDel?.device ?? "";
    const paths = confirmDel?.paths ?? [];
    setConfirmDel(null);
    if (device === "" || paths.length === 0) return;
    deletingRef.current = true;
    void deletePaths(device, paths, true).finally(() => {
      deletingRef.current = false;
    });
  }, [confirmDel, deletePaths]);

  /// 瞬时提示自动消隐(6s),换提示即重置计时。
  useEffect(() => {
    if (notice == null) return;
    const timer = window.setTimeout(() => setNotice(null), 6000);
    return () => window.clearTimeout(timer);
  }, [notice]);

  // ---------- 操作面 II(Task 12):剪贴板 / 粘贴分派 / 冲突 / 下载 / 拖拽 ----------

  /// 瞬时错误提示的统一整形(friendlyError 信号 → 文案)。
  const notifyError = useCallback(
    (e: unknown) => {
      const fe = friendlyError(e);
      setNotice({ text: fe.kind === "offline" ? t(fe.text) : fe.text, tone: "error" });
    },
    [t],
  );

  /// 计划执行体:按 kind 发 RPC。device_copy 走 ctrl copy/move(即时,无
  /// 作业卡;op 材料由 planSameDeviceOp 装配经 sdArgs 传入,policy 为已换
  /// 算判定键的冲突表——不经 planSameDeviceOp 的 device_copy 调用不受理;
  /// 仅远端同设备走此面);local_copy 走 transfer.start 本机复制作业(M4b
  /// 移交债①回迁,sdArgs 不消费——参数自 plan 材料装配,卡片由 event.
  /// transfer 事件驱动出现);其余 transfer 形态 transfer.start 起作业,
  /// **卡片均由 event.transfer 事件驱动出现**(Task 13,此处不再发「已开
  /// 始」提示)。剪切粘贴成功后清内部剪贴板(资源管理器同款)。目标为当
  /// 前目录时顺带刷新列表。
  const executePlan = useCallback(
    async (
      plan: PastePlan,
      policy: { skip: string[]; overwrite: string[] },
      targetDevice: string,
      targetDir: string,
      sdArgs?: SameDeviceArgs,
    ): Promise<void> => {
      try {
        switch (plan.kind) {
          case "upload":
            await daemonTransferStart({
              kind: "upload", files: plan.files, to: plan.to, dir: plan.dir,
              skip: policy.skip, overwrite: policy.overwrite, cut: plan.cut,
            } satisfies TransferStartParams);
            break;
          case "download":
            await daemonTransferStart({
              kind: "download", device: plan.device, paths: plan.paths, dir: plan.dir,
              skip: policy.skip, overwrite: policy.overwrite, cut: plan.cut,
            } satisfies TransferStartParams);
            break;
          case "remote_copy":
            await daemonTransferStart({
              kind: "remote_copy", from: plan.from, paths: plan.paths, to: plan.to, dir: plan.dir,
              skip: policy.skip, overwrite: policy.overwrite, cut: plan.cut,
            } satisfies TransferStartParams);
            break;
          case "device_copy": {
            // 同机 ctrl 面(Task 4):op 材料(copy → paths+dir;move →
            // rename 对)由 planSameDeviceOp 装配;policy 已换算为引擎判定
            // 键(目标目录拼名 / move 的 to 原串),空表 = 无冲突直发/
            // legacy。逐条失败表以提示呈现;daemon 回带的 skipped 名单暂
            // 不额外呈现(决策已在对话框落定,跳过项静默与传输面同口径)。
            if (sdArgs == null) return; // 防御:同机面必经 planSameDeviceOp 材料
            const r = sdArgs.op === "copy"
              ? await daemonCopy(plan.device, sdArgs.paths, sdArgs.dir, policy)
              : await daemonMove(plan.device, sdArgs.pairs, policy);
            if (r.failed.length > 0) {
              setNotice({
                text: t("transfer.deviceCopyFailed", { message: r.failed[0].message }),
                tone: "error",
              });
            }
            break;
          }
          case "local_copy":
            // 本机复制作业面(M4b 移交债①回迁):自 plan 材料装配参数起
            // transfer.start 作业,policy 为 planSameDeviceOp 换算后的判定
            // 键表(与 ctrl 面同规则;空表 = 无冲突直发/legacy)。卡片/
            // 进度/跳过计数由 event.transfer 管线呈现;sdArgs(ctrl 面
            // 材料)在此形态不消费。
            await daemonTransferStart(localCopyStartParams(plan, policy));
            break;
          case "none":
            return;
        }
        if (plan.cut) setClip(null); // 剪切粘贴已发起,源删除由 daemon 按 cut 收尾
        if (plan.kind !== "device_copy") {
          mergeTransfers(); // 事件帧不带 src/dst:起作业即拉一次 list 补齐卡片路径面
        }
        // 目标为当前目录(目录位原样;favdir 位=joinRel 原生路径,返工
        // §13.6)时顺带刷新列表。
        const cur = nav.current;
        const isHere =
          (cur.kind === "dir" && cur.device === targetDevice && cur.path === targetDir) ||
          (cur.kind === "favdir" &&
            cur.device === targetDevice &&
            joinRel(cur.base, cur.rel) === targetDir);
        if (isHere) {
          setReloadTick((x) => x + 1);
        }
      } catch (e) {
        notifyError(e);
      }
    },
    [nav.current, notifyError, t, mergeTransfers],
  );

  /// 分派核心:预扫形态(upload/download/remote_copy/device_copy/
  /// local_copy)先取目标目录列表(daemonList;NOT_FOUND = 目标目录尚不
  /// 存在 = 无冲突;其余失败 → 提示并中止,不盲发);预扫命中 → 挂冲突
  /// 对话框(四键裁决,目录恒合并已在 scanConflicts 滤除),无冲突 →
  /// 直接执行。同设备两臂(Task 4)经 planSameDeviceOp 装配材料并把裁决
  /// rel 换算为引擎判定键(copy = 目标目录拼名 / move = to 原串):远端
  /// 同设备 device_copy 换算后的键面进 ctrl copy/move;本机 local_copy
  /// (M4b 移交债①回迁)同键面进 transfer.start 策略参数——对话框与
  /// 执行共用同一份材料,两臂零漂移。
  const runTransfer = useCallback(
    async (
      plan: PastePlan,
      sources: FsEntry[],
      targetDevice: string,
      targetDir: string,
    ): Promise<void> => {
      const hasPolicy =
        plan.kind === "upload" ||
        plan.kind === "download" ||
        plan.kind === "remote_copy" ||
        plan.kind === "device_copy" ||
        plan.kind === "local_copy";
      const policy = { skip: [] as string[], overwrite: [] as string[] };
      if (hasPolicy) {
        let entries: FsEntry[] = [];
        try {
          entries = (await daemonList(targetDevice, targetDir)).entries;
        } catch (e) {
          // NOT_FOUND(data.kind 类型化,M4a-7 起;标记串子串匹配已废)
          // = 目标目录尚不存在(接收端会建出)= 无冲突;其余失败如实提示。
          if (kind(toRpcError(e)) !== "NOT_FOUND") {
            // 目标目录查询失败(离线/权限等):无法判断冲突,如实提示后中止。
            const fe = friendlyError(e);
            setNotice({
              text: t("files.preScanFailed", {
                message: fe.kind === "offline" ? t(fe.text) : fe.text,
              }),
              tone: "error",
            });
            return;
          }
        }
        if (plan.kind === "device_copy" || plan.kind === "local_copy") {
          // 同设备两臂(Task 4;local_copy M4b 回迁):planSameDeviceOp
          // 一份材料三用——冲突行进对话框;无冲突空策略直发;对话框裁决
          // 经 exec(判定键已换算)回到 executePlan。cut 标志定形态(窗
          // 内拖拽恒 move,粘贴看 cut)。local_copy 的换算后策略键与 ctrl
          // 面同规则,executePlan 自 plan 材料起作业(材料仅作预扫)。
          const sd = planSameDeviceOp(
            plan.cut ? "move" : "copy",
            sources,
            targetDir,
            entries,
            (args, p) => void executePlan(plan, p, targetDevice, targetDir, args),
          );
          if (sd.conflicts.length > 0) {
            conflictParkedRef.current = true; // 冲突挂起(downloadMixed 判停读)
            setPendingConflict({
              conflicts: sd.conflicts,
              exec: (p) => {
                setPendingConflict(null);
                sd.exec(p);
              },
            });
            return;
          }
          sd.exec(policy);
          return;
        }
        const conflicts = scanConflicts(sources, entries);
        if (conflicts.length > 0) {
          conflictParkedRef.current = true; // 冲突挂起(downloadMixed 判停读)
          setPendingConflict({
            conflicts,
            exec: (p) => {
              setPendingConflict(null);
              void executePlan(plan, p, targetDevice, targetDir);
            },
          });
          return;
        }
      }
      await executePlan(plan, policy, targetDevice, targetDir);
    },
    [executePlan, t],
  );

  /// 本机源条目的冲突预扫元数据:逐条 stat(daemon 本机短路直调,毫秒级;
  /// 失败条目从预扫面剔除,不放大——transfer.start 的同步校验自会拒空)。
  /// 返回完整 FsEntry(Task 4:同机面的执行材料还要取 path 拼参数)。
  const statLocalSources = useCallback(async (files: string[]): Promise<FsEntry[]> => {
    if (selfDeviceId == null) return [];
    const out: FsEntry[] = [];
    for (const f of files) {
      try {
        out.push(await daemonStat(selfDeviceId, f));
      } catch {
        // 预扫降级:该条目无元数据(嵌套同名交引擎自动改名安全网)
      }
    }
    return out;
  }, [selfDeviceId]);

  /// 剪切/复制当前选择进内部剪贴板(条目元数据随行,冲突预扫免二次
  /// stat;Ctrl+X/Ctrl+C/工具栏/右键菜单共用)。MTDSS 不写系统剪贴板。
  /// favdir 位(返工 §13.6)同款 as-dir(设备=成员设备)。
  const clipFromSelection = useCallback(
    (cut: boolean) => {
      const cur = nav.current;
      if ((cur.kind !== "dir" && cur.kind !== "favdir") || selection.size === 0) return;
      const entries = rows.filter((r) => selection.has(r.path));
      if (entries.length === 0) return;
      setClip({ device: cur.device, entries, cut });
    },
    [nav.current, selection, rows],
  );

  /// 粘贴可用门控:目录位 = 非根卷列表(path="" 为卷列表,非文件夹)+
  /// 设备在线;快捷命名空间位(返工 §13.6,三视图 as-dir)= 设备在线;
  /// 快捷位(V3-3 T12)= 当前快捷存在且有成员(多成员开选择层,单成员
  /// 直进)。可用性不看内部剪贴板是否为空——空则转读系统 CF_HDROP
  /// (可能恰有 Explorer 复制的文件)。
  const canPaste =
    (nav.current.kind === "dir" && nav.current.path !== "" && !browsingOffline) ||
    (nav.current.kind === "favdir" && !browsingOffline) ||
    (nav.current.kind === "fav" && (currentFav?.members.length ?? 0) > 0);

  /// 粘贴执行体(T12 从 requestPaste 抽出,目标已知):内部剪贴板非空 →
  /// pastePlan 五态分派;空 → 桥读系统 CF_HDROP → upload(系统剪贴板恒
  /// 视为复制)。dir 位(requestPaste)与 fav 位成员解析(requestPasteFav
  /// /成员选择层 onPick)共用;根卷列表/离线/冲突占线等门控由调用方负责。
  const pasteInto = useCallback(
    (device: string, path: string) => {
      if (clip != null && clip.entries.length > 0) {
        const plan = pastePlan(
          { device: clip.device, paths: clip.entries.map((e) => e.path), cut: clip.cut },
          selfDeviceId,
          device,
          path,
        );
        if (plan.kind === "none") return;
        void runTransfer(plan, clip.entries, device, path);
        return;
      }
      // 空内部剪贴板:系统 CF_HDROP → upload(Explorer 复制 → MTDSS 粘贴)。
      void (async () => {
        let files: string[] = [];
        try {
          files = await clipboardFiles();
        } catch {
          return; // 桥失败 = 无法判定,静默(与空剪贴板同表现)
        }
        if (files.length === 0) return; // 无文件剪贴板:粘贴 no-op
        const sources = await statLocalSources(files);
        void runTransfer(
          { kind: "upload", files, to: device, dir: path, cut: false },
          sources,
          device,
          path,
        );
      })();
    },
    [clip, selfDeviceId, runTransfer, statLocalSources],
  );

  /// 粘贴入口(目录位/快捷命名空间位:工具栏/右键菜单/Ctrl+V 共用):
  /// requestPaste 瘦壳——门控(根卷列表/离线)后交 pasteInto。favdir 位
  /// (返工 §13.6)目标=当前命名空间原生路径(joinRel),三视图 as-dir
  /// 零新动作码。
  const requestPaste = useCallback(() => {
    const cur = nav.current;
    if (cur.kind !== "dir" && cur.kind !== "favdir") return;
    if (pendingConflict != null) return;
    const device = cur.device;
    const path = cur.kind === "dir" ? cur.path : joinRel(cur.base, cur.rel);
    if (path === "") {
      setNotice({ text: t("files.cannotPasteRoot"), tone: "error" });
      return;
    }
    if (browsingOffline) {
      setNotice({ text: t("files.errOffline"), tone: "error" });
      return;
    }
    pasteInto(device, path);
  }, [nav.current, browsingOffline, pendingConflict, pasteInto, t]);

  /// 粘贴入口(快捷位,T12):落点解析与拖放同语义(resolveFavDropTarget)
  /// ——单成员直进 pasteInto;多成员开成员选择层(favMemberPasteFor),点选
  /// 后 pasteInto;空快捷(0 成员)静默忽略(canPaste 已禁,防御)。目标
  /// 成员离线 → 「设备离线」提示(目录位离线门同款,先说清不发 RPC)。
  const requestPasteFav = useCallback(() => {
    if (nav.current.kind !== "fav" || currentFav == null) return;
    if (pendingConflict != null) return;
    const target = resolveFavDropTarget(currentFav);
    if (target == null) return;
    if (target.kind === "picker") {
      setFavMemberPasteFor({ favId: currentFav.id });
      return;
    }
    if (!deviceOnline(target.member.device)) {
      setNotice({ text: t("files.errOffline"), tone: "error" });
      return;
    }
    pasteInto(target.member.device, target.member.path);
  }, [nav.current, currentFav, pendingConflict, deviceOnline, pasteInto, t]);

  /// 「下载到本机…」(右键菜单,仅远端设备条目——本机文件本就在本机):
  /// 目录选择器 → download 作业(冲突预扫对本机目录,daemonList(self)
  /// 短路本地 fs;NOT_FOUND = 空)。取消选择器即取消操作。favdir 位
  /// (返工 §13.6)同款 as-dir:行恒属成员单设备,远端才呈现/生效。
  const requestDownloadToLocal = useCallback(() => {
    const cur = nav.current;
    if ((cur.kind !== "dir" && cur.kind !== "favdir") || selection.size === 0) return;
    if (selfDeviceId == null || cur.device === selfDeviceId) return;
    const entries = rows.filter((r) => selection.has(r.path));
    if (entries.length === 0) return;
    const device = cur.device;
    void (async () => {
      let dir: string | null = null;
      try {
        dir = await pickFolder();
      } catch {
        return; // 选择器失败:静默取消
      }
      if (dir == null) return;
      void runTransfer(
        { kind: "download", device, paths: entries.map((e) => e.path), dir, cut: false },
        entries,
        selfDeviceId,
        dir,
      );
    })();
  }, [nav.current, selection, rows, selfDeviceId, runTransfer]);

  /// ---- 快捷位动作对等(V3-3 Task 11,复合键选择面)----
  /// 动作处理器统一收 `ReadonlySet<string>`(fav 位复合键选择集,T12 复
  /// 用);跨源分组一律以 favRowsRef 现值(渲染行上抛)计算——渐进扇出中
  /// 调用时点所见即所选。

  /// fav 位「下载到本机…」(跨源分组汇聚):目录选择器一次 → 逐组发起
  /// 传输(dir 位 runTransfer 同款:冲突预扫对本机目录)。本机组不再静默
  /// 跳过(返工 §14.2 修复:旧「本机文件已在本机」跳过在部分本机选中时
  /// 静默丢组 =「跨成员多选下载只下一个」根因)——本机组转 local_copy
  /// 本机复制作业复制进所选目录,远端组 download,分派决策经
  /// downloadMixedPlans 纯函数钉死(双组双发)。取消选择器 = no-op。推进
  /// 经 runGroupsWithConflictPause(T11 评审 Important #1):冲突挂起即停,
  /// 其余组不发起(防后组冲突批顶掉前组待决批的静默丢组),rest>0 →
  /// toast「其余 N 组未发起,请先处理冲突」(单组冲突 rest=0 不提示)。
  /// 分组映射为空(选择键无行命中,防御面)→ 静默取消(dropSourcesTo
  /// 全剔除同款)。
  const downloadMixed = useCallback(
    (sel: ReadonlySet<string>) => {
      if (nav.current.kind !== "fav" || selfDeviceId == null || sel.size === 0) return;
      const groups = groupRowsByDevice(favRowsRef.current, sel);
      void (async () => {
        let dir: string | null = null;
        try {
          dir = await pickFolder();
        } catch {
          return; // 选择器失败:静默取消(目录位同款)
        }
        if (dir == null) return;
        const targetDir: string = dir; // 窄化出闭包(let 属性窄化不跨函数边界)
        const plans = downloadMixedPlans(groups, selfDeviceId, targetDir);
        if (plans.length === 0) return; // 分组映射为空(选择键无行命中):静默取消
        conflictParkedRef.current = false;
        const { rest } = await runGroupsWithConflictPause(
          plans,
          (p) => runTransfer(p.plan, p.source.entries, selfDeviceId, targetDir),
          () => conflictParkedRef.current,
        );
        if (rest > 0) {
          pushToast({
            title: t("transfer.mixedConflictPaused", { count: rest }),
            body: "",
            tone: "info",
          });
        }
      })();
    },
    [nav.current, selfDeviceId, pushToast, t, runTransfer],
  );

  /// fav 位复制/剪切(单源门):分组数 !==1(跨源多选)→ toast 阻断;
  /// 单源组 → 现有 clip 形态零改落库。裁定记档:cut 源行灰显在 fav 位不
  /// 命中(cutPaths 是 path 集,fav 行键是复合键),视觉缺失接受,T12 不做。
  const copyCutMixed = useCallback(
    (sel: ReadonlySet<string>, cut: boolean) => {
      if (nav.current.kind !== "fav" || sel.size === 0) return;
      const groups = groupRowsByDevice(favRowsRef.current, sel);
      if (groups.length !== 1) {
        pushToast({ title: t("favorites.crossSourceBlocked"), body: "", tone: "error" });
        return;
      }
      setClip({ device: groups[0].device, entries: groups[0].entries, cut });
    },
    [nav.current, pushToast, t],
  );

  /// 多源拖放落点执行体(V3-3 T12):把源组集扇出到单一目标(device\0path
  /// 键、快捷单成员解析、成员选择层点选三路汇此)。逐组 planMultiSourceDrop
  /// 规划(原地放置组/空计划组静默剔除——单源 dir 位会话的「原地放置 no-op」
  /// 语义逐字节保持),全组剔除即静默取消;再查目标设备在线(目录位既有
  /// 「先说清」门,剔除之后才查,顺序与抽取前一致);推进经
  /// runGroupsWithConflictPause(T11 同款:冲突挂起即停,余组计数 toast)
  /// 逐组 runTransfer。单组场景 rest 恒 0,与既有单源行为无感对齐。
  const dropSourcesTo = useCallback(
    (sources: DragSourceSel[], targetDevice: string, targetDir: string) => {
      const plans = planMultiSourceDrop(sources, selfDeviceId, targetDevice, targetDir);
      if (plans.length === 0) return; // 全组原地放置/无计划:静默(原地 no-op)
      if (!deviceOnline(targetDevice)) {
        setNotice({ text: t("files.errOffline"), tone: "error" });
        return;
      }
      void (async () => {
        conflictParkedRef.current = false;
        const { rest } = await runGroupsWithConflictPause(
          plans,
          (p) => runTransfer(p.plan, p.source.entries, targetDevice, targetDir),
          () => conflictParkedRef.current,
        );
        if (rest > 0) {
          pushToast({
            title: t("transfer.mixedConflictPaused", { count: rest }),
            body: "",
            tone: "info",
          });
        }
      })();
    },
    [selfDeviceId, deviceOnline, runTransfer, pushToast, t],
  );

  /// 成员选择层点选收口(T12):拖放批 → dropSourcesTo 落所选成员;粘贴批
  /// → pasteInto。双批互斥(一次只开一场),收口即双清。
  const handleFavMemberPick = useCallback(
    (member: FavMember) => {
      const drop = favMemberDropFor;
      const paste = favMemberPasteFor;
      setFavMemberDropFor(null);
      setFavMemberPasteFor(null);
      if (drop != null) {
        dropSourcesTo(drop.sources, member.device, member.path);
      } else if (paste != null) {
        pasteInto(member.device, member.path);
      }
    },
    [favMemberDropFor, favMemberPasteFor, dropSourcesTo, pasteInto],
  );

  /// fav 位删除执行体(多组回收梯,dir 位 deletePaths 的多组循环形态):
  /// 逐组回收优先直接 RPC(可回收静默);任一组 need_force(回收站不可用)
  /// → 挂起 favDel 多组确认梯(**仅收 need_force 组**——其余组已回收成功),
  /// 确认后整批 force 重发;code 3 离线 → 友好提示;部分失败如实呈现(首个
  /// 失败组),任何形状的成功后清选择并重扇出(reloadTick)。
  const deleteGroups = useCallback(
    async (groups: { device: string; paths: string[] }[], force: boolean): Promise<void> => {
      const needForce: { device: string; paths: string[] }[] = [];
      let firstFail: string | null = null;
      for (const g of groups) {
        try {
          const r = await daemonDelete(g.device, g.paths, force);
          if (r.failed.length > 0 && firstFail == null) {
            firstFail = t("files.deleteFailed", { message: r.failed[0].message });
          }
        } catch (err) {
          const fe = friendlyError(err);
          if (fe.kind === "need_force") needForce.push(g);
          else if (firstFail == null) firstFail = fe.kind === "offline" ? t(fe.text) : fe.text;
        }
      }
      if (needForce.length > 0) {
        setFavDel({ groups: needForce });
        return;
      }
      if (firstFail != null) setNotice({ text: firstFail, tone: "error" });
      setSelection(new Set());
      setAnchorPath(null);
      setReloadTick((x) => x + 1);
    },
    [t],
  );

  /// fav 位删除入口(工具栏/右键菜单/Del 键共用):非空选择 + 在途防重入
  /// (deletingRef 与目录位共用,全局面互斥)。
  const deleteMixed = useCallback(
    (sel: ReadonlySet<string>) => {
      if (deletingRef.current || nav.current.kind !== "fav" || sel.size === 0) return;
      const groups = groupRowsByDevice(favRowsRef.current, sel).map((g) => ({
        device: g.device,
        paths: g.entries.map((e) => e.path),
      }));
      if (groups.length === 0) return;
      deletingRef.current = true;
      void deleteGroups(groups, false).finally(() => {
        deletingRef.current = false;
      });
    },
    [deleteGroups, nav.current],
  );

  /// fav 位确认梯终步:force=true 永久删除 need_force 组(开梯快照重发,
  /// 与目录位 confirmForceDelete 同口径;设备取快照,不读当前导航)。
  const confirmFavForceDelete = useCallback(() => {
    const groups = favDel?.groups ?? [];
    setFavDel(null);
    if (groups.length === 0) return;
    deletingRef.current = true;
    void deleteGroups(groups, true).finally(() => {
      deletingRef.current = false;
    });
  }, [favDel, deleteGroups]);

  /// Del 键删除:输入框聚焦不劫持;任一模态(右键菜单/删除确认/冲突
  /// 对话框,modalBusy)打开时不触发——评审 Important #1:曾漏挡
  /// pendingConflict,Del 会在冲突对话框遮罩下删掉当前选择;非空
  /// 选择且处于目录位才发起(离线设备照发,RPC code 3 走友好提示——
  /// 即「离线删除报友好文案」冒烟路径)。fav 位(V3-3 T11)非空选择
  /// 走 deleteMixed 多组回收梯。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Delete") return;
      const tgt = e.target as HTMLElement | null;
      if (
        tgt != null &&
        (tgt.tagName === "INPUT" || tgt.tagName === "TEXTAREA" || tgt.isContentEditable)
      ) {
        return;
      }
      if (modalBusy({ ctxMenu, confirmDel, pendingConflict, updatePage: updatePageOpen, approvalModal: approvalReq, favName: favNameOpen, favDel, favPick: favPickFor, favMemberPick: memberPickFav, favMenu: menuFav, favManage: manageFav, favDelFor })) return;
      if (nav.current.kind === "fav") {
        if (selection.size === 0) return;
        e.preventDefault();
        deleteMixed(selection);
        return;
      }
      if (nav.current.kind !== "dir" && nav.current.kind !== "favdir") return;
      const paths = [...selection];
      if (paths.length === 0) return;
      e.preventDefault();
      requestDelete(paths);
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [selection, ctxMenu, confirmDel, pendingConflict, updatePageOpen, approvalReq, favNameOpen, favDel, favPickFor, memberPickFav, menuFav, manageFav, favDelFor, nav.current, requestDelete, deleteMixed]);

  /// Ctrl+X / Ctrl+C / Ctrl+V:输入框聚焦不劫持;任一模态打开(modalBusy,
  /// 与 Del 键同一守卫)不触发;目录位才生效(V 与粘贴入口同一分派)。
  /// fav 位(V3-3 T11):X/C 走 copyCutMixed 单源门;V 走 fav 位粘贴
  /// (T12:多成员开成员选择层,单成员直进,与拖放同语义)。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.ctrlKey && !e.metaKey) return;
      const k = e.key.toLowerCase();
      if (k !== "x" && k !== "c" && k !== "v") return;
      const tgt = e.target as HTMLElement | null;
      if (
        tgt != null &&
        (tgt.tagName === "INPUT" || tgt.tagName === "TEXTAREA" || tgt.isContentEditable)
      ) {
        return;
      }
      if (modalBusy({ ctxMenu, confirmDel, pendingConflict, updatePage: updatePageOpen, approvalModal: approvalReq, favName: favNameOpen, favDel, favPick: favPickFor, favMemberPick: memberPickFav, favMenu: menuFav, favManage: manageFav, favDelFor })) return;
      if (nav.current.kind === "fav") {
        if (k === "v") {
          e.preventDefault();
          requestPasteFav();
          return;
        }
        if (selection.size === 0) return;
        e.preventDefault();
        copyCutMixed(selection, k === "x");
        return;
      }
      if (nav.current.kind !== "dir" && nav.current.kind !== "favdir") return;
      if (k === "v") {
        e.preventDefault();
        requestPaste();
        return;
      }
      if (selection.size === 0) return;
      e.preventDefault();
      clipFromSelection(k === "x");
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [selection, ctxMenu, confirmDel, pendingConflict, updatePageOpen, approvalReq, favNameOpen, favDel, favPickFor, memberPickFav, menuFav, manageFav, favDelFor, nav.current, requestPaste, requestPasteFav, clipFromSelection, copyCutMixed]);

  /// 拖入(外部文件 → 窗口):Tauri onDragDropEvent(dragDropEnabled=true
  /// 才有真实路径)。目录位:整窗即当前目录(非根卷列表且设备在线才受理;
  /// 设备视图/根卷位拖入无目标,V2 既有行为逐字节保留)。快捷位(V3-3
  /// T12):命中测试定落点——elementFromPoint 找 [data-drop-loc](侧栏
  /// 快捷行 fav\0id / 收件箱行、混合视图宿主、分组节头 device\0path),
  /// fav 键走 resolveFavDropTarget 单成员直传/多成员开选择层,device 键
  /// 直传该目标(离线拒,终审修⑤),无命中忽略(注意:onDragDropEvent 的 position 是**物理
  /// 像素**,elementFromPoint 吃 CSS 像素,须除以 devicePixelRatio 换算
  /// ——Review Focus #4,高分屏不除则命中点整体偏移)。冲突流与粘贴同一
  /// runTransfer。模态占线时同样不受理(评审补丁):冲突对话框开着时,
  /// Explorer 拖入若直接 runTransfer 会顶掉待决批次、静默取消前一次粘贴。
  useEffect(() => {
    const kind = nav.current.kind;
    if (kind === "dir" && (nav.current.path === "" || browsingOffline)) return;
    // favdir 位(返工 §13.6):整窗=当前命名空间目录(as-dir;离线同门)。
    if (kind === "favdir" && browsingOffline) return;
    if (kind !== "dir" && kind !== "fav" && kind !== "favdir") return;
    const device =
      nav.current.kind === "dir" || nav.current.kind === "favdir" ? nav.current.device : "";
    const path =
      nav.current.kind === "dir"
        ? nav.current.path
        : nav.current.kind === "favdir"
          ? joinRel(nav.current.base, nav.current.rel)
          : "";
    const unlisten = getCurrentWindow().onDragDropEvent((ev) => {
      if (ev.payload.type !== "drop") return;
      if (modalBusy({ ctxMenu, confirmDel, pendingConflict, updatePage: updatePageOpen, approvalModal: approvalReq, favName: favNameOpen, favDel, favPick: favPickFor, favMemberPick: memberPickFav, favMenu: menuFav, favManage: manageFav, favDelFor })) return;
      const files = ev.payload.paths;
      if (files.length === 0) return;
      // drop 帧的光标位置先取出(联合类型收窄不进闭包;物理像素,物理→
      // CSS 换算见上注)。
      const pos = ev.payload.type === "drop" ? ev.payload.position : null;
      void (async () => {
        if (kind === "dir" || kind === "favdir") {
          const sources = await statLocalSources(files);
          void runTransfer(
            { kind: "upload", files, to: device, dir: path, cut: false },
            sources,
            device,
            path,
          );
          return;
        }
        // 快捷位:命中测试先行(同步,无命中不做无谓 stat)。
        if (pos == null) return;
        const scale = window.devicePixelRatio > 0 ? window.devicePixelRatio : 1;
        const el = document
          .elementFromPoint(pos.x / scale, pos.y / scale)
          ?.closest("[data-drop-loc]");
        const key = el?.getAttribute("data-drop-loc") ?? null;
        if (key == null) return; // 无落点(空白/工具栏等):忽略
        // 命中后取源条目元数据(冲突预扫材料;Explorer 源恒为本机)。
        const sources = await statLocalSources(files);
        if (key.startsWith("fav\u0000")) {
          const fav = favStore.state.doc.favorites.find((f) => f.id === key.slice(4));
          if (fav == null) return; // 清单已变(他端删除随 data_changed 同步)
          const target = resolveFavDropTarget(fav);
          if (target == null) return; // 空快捷(0 成员):无落点
          // 多成员 → 开成员选择层(源=本机上传形态,srcDir=null);点选后
          // 与单成员同路(upload 到所选成员)。selfDeviceId 缺(未登录/无
          // hello)= 无从谈起本机源,直接 return 不造伪设备(T19 修④)。
          if (target.kind === "picker") {
            if (selfDeviceId == null) return;
            setFavMemberDropFor({
              favId: fav.id,
              sources: [{ device: selfDeviceId, entries: sources, srcDir: null }],
            });
            return;
          }
          // 单成员:目标离线 → 「设备离线」提示(内部拖/粘贴两路同门,
          // T19 修②补齐,先说清不发 RPC)。
          if (!deviceOnline(target.member.device)) {
            setNotice({ text: t("files.errOffline"), tone: "error" });
            return;
          }
          void runTransfer(
            { kind: "upload", files, to: target.member.device, dir: target.member.path, cut: false },
            sources,
            target.member.device,
            target.member.path,
          );
          return;
        }
        const nul = key.indexOf("\u0000");
        const toDevice = key.slice(0, nul);
        const toDir = key.slice(nul + 1);
        // 落点设备离线 → 「设备离线」提示不发(与单成员快捷支 T19 修②
        // 同门,终审修⑤补齐:分组节头/收件箱行 device 键原漏此门)。
        if (!deviceOnline(toDevice)) {
          setNotice({ text: t("files.errOffline"), tone: "error" });
          return;
        }
        void runTransfer(
          { kind: "upload", files, to: toDevice, dir: toDir, cut: false },
          sources,
          toDevice,
          toDir,
        );
      })();
    });
    return () => {
      void unlisten.then((u) => u());
    };
  }, [dirKey, nav.current, browsingOffline, ctxMenu, confirmDel, pendingConflict, updatePageOpen, approvalReq, favNameOpen, favDel, favPickFor, memberPickFav, menuFav, manageFav, favDelFor, favStore.state.doc.favorites, selfDeviceId, statLocalSources, runTransfer, deviceOnline, t]);

  /// 拖出窗口松开:drop_resolve_at_cursor(Task 10)→ 桌面/资源管理器目录
  /// → download 直落该目录;unresolvable → 钉死提示「请拖到桌面或使用
  /// 下载到本机…」(解析器理由非空时附在提示后,便于核对落点被拒因由)。
  /// 本机行拖出无意义(download 拒自源),直接忽略。
  const resolveExternalDrop = useCallback(
    (device: string, entries: FsEntry[]) => {
      // 模态占线不受理(与拖入/粘贴同门;OS 级落点不受 DOM 遮罩拦截)。
      if (modalBusy({ ctxMenu, confirmDel, pendingConflict, updatePage: updatePageOpen, approvalModal: approvalReq, favName: favNameOpen, favDel, favPick: favPickFor, favMemberPick: memberPickFav, favMenu: menuFav, favManage: manageFav, favDelFor })) return;
      if (selfDeviceId == null || device === selfDeviceId) return;
      void (async () => {
        let r: DropResolveResult;
        try {
          r = await dropResolveAtCursor();
        } catch {
          setNotice({ text: t("transfer.dropUnresolvable"), tone: "error" });
          return;
        }
        if (r.kind === "unresolvable" || r.path == null) {
          // 附上解析器的拒绝理由(zh 人可读),便于冒烟核对落点为何被拒。
          const text =
            r.kind === "unresolvable" && r.reason
              ? `${t("transfer.dropUnresolvable")}(${r.reason})`
              : t("transfer.dropUnresolvable");
          setNotice({ text, tone: "error" });
          return;
        }
        void runTransfer(
          { kind: "download", device, paths: entries.map((e) => e.path), dir: r.path, cut: false },
          entries,
          selfDeviceId,
          r.path,
        );
      })();
    },
    [ctxMenu, confirmDel, pendingConflict, updatePageOpen, approvalReq, favNameOpen, favDel, favPickFor, memberPickFav, menuFav, manageFav, favDelFor, selfDeviceId, runTransfer, t],
  );

  /// fav 位拖出窗口松开(V3-3 T19 收尾批修①,补 T12 遗留的静默忽略面):
  /// 落点解析一次(与 dir 位同一 drop_resolve_at_cursor 契约,多组只问一
  /// 次),分派决策与菜单下载流同源(downloadMixedPlans,S5 修复轮 §14.2
  /// 同根因收敛)——远端组 download 直落该目录,本机组 local_copy 复制进
  /// 该目录(不再「本机文件已在本机」静默跳过,混合拖出双组双发),cut
  /// 恒 false;空源组(防御)静默;unresolvable/解析失败 → 既有钉死提示
  /// (对 fav 位同样适用)。推进经 runGroupsWithConflictPause(同款冲突
  /// 停:park 即停 + rest 计数 toast,单组时无感)。
  const resolveExternalDropFav = useCallback(
    (sources: DragSourceSel[]) => {
      // 模态占线不受理(与 resolveExternalDrop 同门;OS 级落点不受 DOM
      // 遮罩拦截)。
      if (modalBusy({ ctxMenu, confirmDel, pendingConflict, updatePage: updatePageOpen, approvalModal: approvalReq, favName: favNameOpen, favDel, favPick: favPickFor, favMemberPick: memberPickFav, favMenu: menuFav, favManage: manageFav, favDelFor })) return;
      if (selfDeviceId == null || sources.length === 0) return;
      void (async () => {
        let r: DropResolveResult;
        try {
          r = await dropResolveAtCursor();
        } catch {
          setNotice({ text: t("transfer.dropUnresolvable"), tone: "error" });
          return;
        }
        if (r.kind === "unresolvable" || r.path == null) {
          // 附上解析器的拒绝理由(zh 人可读),便于冒烟核对落点为何被拒。
          const text =
            r.kind === "unresolvable" && r.reason
              ? `${t("transfer.dropUnresolvable")}(${r.reason})`
              : t("transfer.dropUnresolvable");
          setNotice({ text, tone: "error" });
          return;
        }
        const dir = r.path; // 窄化出闭包(let 属性窄化不跨函数边界)
        // DragSourceSel 结构满足 SourceSel(device+entries;srcDir 为拖拽
        // 会话额外位)——两流共用同一分派纯函数,永不漂移出两套语义。
        const plans = downloadMixedPlans(sources, selfDeviceId, dir);
        if (plans.length === 0) return; // 空映射(防御):静默取消
        conflictParkedRef.current = false;
        const { rest } = await runGroupsWithConflictPause(
          plans,
          (p) => runTransfer(p.plan, p.source.entries, selfDeviceId, dir),
          () => conflictParkedRef.current,
        );
        if (rest > 0) {
          pushToast({
            title: t("transfer.mixedConflictPaused", { count: rest }),
            body: "",
            tone: "info",
          });
        }
      })();
    },
    [ctxMenu, confirmDel, pendingConflict, updatePageOpen, approvalReq, favNameOpen, favDel, favPickFor, memberPickFav, menuFav, manageFav, favDelFor, selfDeviceId, pushToast, t, runTransfer],
  );

  // ---- 指针拖拽(窗口内拖 + 拖出,共用一套;见模块文档实现注) ----
  // 四个处理器为**渲染体内普通函数**(非 useCallback):每次渲染重建,
  // 会话挂载时捕获的是同一批新鲜闭包,收尾按同批身份移除——useCallback
  // 的记忆化在这里反而是身份漂移的根源(挂的是旧身份、移除的是新身份)。

  /// 拖拽会话收尾(统一清理监听与视觉态)。
  function endDragSession(): void {
    dragRef.current = null;
    setDragging(false);
    setDropHoverKey(null);
    setDragGhost(null);
    setDragSourcePaths(null);
    ghostRef.current = null;
    document.removeEventListener("pointermove", onDragPointerMove);
    document.removeEventListener("pointerup", onDragPointerUp);
    document.removeEventListener("pointercancel", onDragPointerCancel);
    window.removeEventListener("blur", onDragBlurCancel);
  }

  /// 拖拽 move:越 4px 阈值才成立(不干扰点击/双击);成立即在 body 上
  /// setPointerCapture——Chromium 捕获语义保证指针拖出窗口后 move/up 仍
  /// 送达(WebView2 同引擎;窗口失焦兜底取消)。窗内时按 elementFromPoint
  /// 命中 [data-drop-loc](树节点/面包屑/当前目录宿主)驱动悬停高亮。
  /// ghost 跟手与拖拽成立时的视觉态(浮卡/源行半透明)也在本处理器落定
  /// (Task 9 纯视觉增量,命中/会话机制零改动)。
  function onDragPointerMove(ev: PointerEvent): void {
    const d = dragRef.current;
    if (d == null || ev.pointerId !== d.pointerId) return;
    // ghost 浮卡跟手(Task 9 视觉层):直接写已挂载节点的 transform,
    // 不经 setState/重渲染;坐标同步记 ref,供浮卡挂载首帧取初值。
    ghostPosRef.current = { x: ev.clientX, y: ev.clientY };
    const ghost = ghostRef.current;
    if (ghost != null) {
      ghost.style.transform = `translate(${ev.clientX + 12}px, ${ev.clientY + 12}px)`;
    }
    if (!d.started) {
      if (Math.hypot(ev.clientX - d.startX, ev.clientY - d.startY) < 4) return;
      d.started = true;
      setDragging(true);
      // 视觉态一次定死:ghost 内容(首项+总数)与源行路径集在整场拖拽
      // 期间不变,收尾时随 endDragSession 统一清。ghost = 首源组首项 +
      // 跨组总数(T12 多源);源行集按会话起步位取键形——dir 位 = 路径集
      // (三视图缺省行键 = path),fav 位 = 复合键集(行 rowKey =
      // device\0path),视图 dragSourcePaths.has(key) 同键比对挂半透明。
      const firstEntry = d.sources[0]?.entries[0];
      if (firstEntry != null) {
        setDragGhost({
          entry: firstEntry,
          count: d.sources.reduce((n, s) => n + s.entries.length, 0),
        });
      }
      setDragSourcePaths(
        d.sources[0]?.srcDir == null
          ? new Set(
              d.sources.flatMap((s) => s.entries.map((e) => `${s.device}\u0000${e.path}`)),
            )
          : new Set(d.sources.flatMap((s) => s.entries.map((e) => e.path))),
      );
      try {
        document.body.setPointerCapture(d.pointerId);
      } catch {
        // 捕获失败(指针已不活动等):退化为普通监听,窗外事件可能丢,
        // blur 兜底收尾
      }
    }
    const outside =
      ev.clientX < 0 ||
      ev.clientY < 0 ||
      ev.clientX > window.innerWidth ||
      ev.clientY > window.innerHeight;
    d.outside = outside;
    if (outside) {
      setDropHoverKey(null);
      return;
    }
    const el = document.elementFromPoint(ev.clientX, ev.clientY)?.closest("[data-drop-loc]");
    const key = el?.getAttribute("data-drop-loc") ?? null;
    setDropHoverKey((prev) => (prev === key ? prev : key));
  }

  /// 拖拽 up:未成立 = 普通点击路径(交回 click 选择);成立后窗内落点
  /// 命中 → 落点键解析分派;窗外松开 → 拖出解析。随后吞一次 click(捕获
  /// 相监听),防拖拽末尾 click 重选/清选。落点键两形(V3-3 T12):
  /// - `device\0path`(树节点/面包屑/当前目录宿主/分组节头/收件箱行)→
  ///   dropSourcesTo 逐源组 dragPlan→runTransfer(同设备=移动/跨设备=复制
  ///   /任何→本机=落盘;原地放置组静默跳过,单源会话语义与多源化前逐字
  ///   节一致);
  /// - `fav\0<id>`(侧栏快捷行/快捷视图宿主)→ resolveFavDropTarget:单
  ///   成员直落该成员(等效 device 键),多成员开成员选择层(源组集挂
  ///   favMemberDropFor),空快捷静默忽略。
  function onDragPointerUp(ev: PointerEvent): void {
    const d = dragRef.current;
    endDragSession();
    if (d == null || ev.pointerId !== d.pointerId) return;
    if (!d.started) return;
    dragMovedRef.current = true;
    if (d.outside) {
      // 拖出窗口松开:dir 位单源会话按既有解析(drop_resolve_at_cursor →
      // download);fav 位会话(组恒 srcDir=null,单/多组)T19 收尾批起
      // 可见化 fan-out——解析一次落点目录,经 downloadMixedPlans 分派(本
      // 机组 local_copy 落盘,S5 修复轮 §14.2 同根因;冲突停同菜单下载
      // 流),不再静默忽略。
      if (d.sources.every((s) => s.srcDir == null)) {
        resolveExternalDropFav(d.sources);
      } else {
        const only = d.sources[0];
        if (only != null && only.srcDir != null) resolveExternalDrop(only.device, only.entries);
      }
      return;
    }
    const el = document.elementFromPoint(ev.clientX, ev.clientY)?.closest("[data-drop-loc]");
    const key = el?.getAttribute("data-drop-loc");
    if (key == null) return; // 无落点(拖到工具栏/侧栏空白等)= 取消
    if (key.startsWith("fav\u0000")) {
      const fav = favStore.state.doc.favorites.find((f) => f.id === key.slice(4));
      if (fav == null) return; // 清单已变(他端删除随 data_changed 同步)
      const target = resolveFavDropTarget(fav);
      if (target == null) return; // 空快捷(0 成员):无落点
      if (target.kind === "picker") {
        setFavMemberDropFor({ favId: fav.id, sources: d.sources });
        return;
      }
      dropSourcesTo(d.sources, target.member.device, target.member.path);
      return;
    }
    const nul = key.indexOf("\u0000");
    dropSourcesTo(d.sources, key.slice(0, nul), key.slice(nul + 1));
  }

  /// 拖拽取消(pointercancel / 窗口失焦兜底):静默弃会话。
  function onDragPointerCancel(_ev: PointerEvent): void {
    endDragSession();
  }

  /// 窗口失焦兜底(指针捕获万一不生效时,窗外松开导致事件流断,靠失焦
  /// 收尾防止会话泄漏)。
  function onDragBlurCancel(): void {
    endDragSession();
  }

  /// 视图宿主 pointerdown(行上,鼠标左键,无修饰键):先清可能泄漏的
  /// 旧会话,再登记新会话并挂 document 级 move/up/cancel + blur 兜底。
  /// 未越阈即松开 = 普通点击,会话就地清除,不干扰现有 click 选择/双击
  /// 打开。源条目集 = 行在选中集 → 整个选中集,否则该行单选(资源管理器
  /// 拖拽同款)。
  const onViewPointerDown = useCallback(
    (e: React.PointerEvent) => {
      if (e.pointerType !== "mouse" || e.button !== 0 || e.shiftKey || e.ctrlKey || e.metaKey) {
        return;
      }
      const cur = nav.current;
      // dir 位与 favdir 位(返工 §13.6)行同可拖(as-dir,单源会话);
      // srcDir = 当前原生路径(favdir = joinRel,原地判定/拖出 fan-out 同源)。
      if (cur.kind !== "dir" && cur.kind !== "favdir") return;
      const rowEl = (e.target as Element).closest("[data-row-path]");
      if (rowEl == null) return;
      const rowPath = rowEl.getAttribute("data-row-path");
      const entry = rows.find((r) => r.path === rowPath);
      if (entry == null) return;
      endDragSession(); // 防御:清掉上一场未收尾的会话与监听
      dragRef.current = {
        pointerId: e.pointerId,
        startX: e.clientX,
        startY: e.clientY,
        sources: [
          {
            device: cur.device,
            srcDir: cur.kind === "dir" ? cur.path : joinRel(cur.base, cur.rel),
            entries: selection.has(entry.path) ? rows.filter((r) => selection.has(r.path)) : [entry],
          },
        ],
        started: false,
        outside: false,
      };
      document.addEventListener("pointermove", onDragPointerMove);
      document.addEventListener("pointerup", onDragPointerUp);
      document.addEventListener("pointercancel", onDragPointerCancel);
      window.addEventListener("blur", onDragBlurCancel);
    },
    // onDragPointerMove/Up/Cancel/Blur 为渲染体普通函数,本回调依赖的
    // 状态(navs/rows/selection)变化即重建,挂载的监听恒为新鲜闭包。
    [nav.current, rows, selection],
  );

  /// 快捷位行 pointerdown(V3-3 T12;FavView 宿主上抛,行命中过滤同
  /// onViewPointerDown):命中行在选中集 → 整个选中集按来源设备分组
  /// (groupRowsByDevice,favRowsRef 现值),否则该行单组(资源管理器
  /// 拖拽同款);组形态 sources=[{device, entries, srcDir:null}]——fav 位
  /// 无单一源目录,原地判定不适用。其余会话机制(越阈/捕获/悬停高亮/
  /// 落点解析)与 dir 位同一套。
  const onFavPointerDown = useCallback(
    (e: React.PointerEvent) => {
      if (e.pointerType !== "mouse" || e.button !== 0 || e.shiftKey || e.ctrlKey || e.metaKey) {
        return;
      }
      if (nav.current.kind !== "fav") return;
      const rowEl = (e.target as Element).closest("[data-row-path]");
      if (rowEl == null) return;
      const rowKey = rowEl.getAttribute("data-row-path");
      if (rowKey == null) return;
      const rowsNow = favRowsRef.current;
      const hit = rowsNow.find((r) => r.key === rowKey);
      if (hit == null) return;
      endDragSession(); // 防御:清掉上一场未收尾的会话与监听
      const sel = selection.has(rowKey) ? selection : new Set<string>([rowKey]);
      dragRef.current = {
        pointerId: e.pointerId,
        startX: e.clientX,
        startY: e.clientY,
        sources: groupRowsByDevice(rowsNow, sel).map((g) => ({
          device: g.device,
          entries: g.entries,
          srcDir: null,
        })),
        started: false,
        outside: false,
      };
      document.addEventListener("pointermove", onDragPointerMove);
      document.addEventListener("pointerup", onDragPointerUp);
      document.addEventListener("pointercancel", onDragPointerCancel);
      window.addEventListener("blur", onDragBlurCancel);
    },
    // onDragPointerMove/Up/Cancel/Blur 为渲染体普通函数,本回调依赖的
    // 状态(nav/selection)变化即重建,挂载的监听恒为新鲜闭包。
    [nav.current, selection],
  );

  /// 拖拽成立后吞掉紧随的 click(捕获相,先于任何冒泡 click 处理器),
  /// 防止松开时的 click 触发行重选/空白清选;吞一次后复位。指针在窗外
  /// 松开时 click 不会派发——标志可能滞留,下一次 pointerdown(捕获相)
  /// 先复位,防误吞后续正常点击。
  useEffect(() => {
    const onDown = () => {
      dragMovedRef.current = false;
    };
    const onClick = (e: MouseEvent) => {
      if (!dragMovedRef.current) return;
      dragMovedRef.current = false;
      e.stopPropagation();
      e.preventDefault();
    };
    document.addEventListener("pointerdown", onDown, true);
    document.addEventListener("click", onClick, true);
    return () => {
      document.removeEventListener("pointerdown", onDown, true);
      document.removeEventListener("click", onClick, true);
    };
  }, []);

  /// 内部剪贴板为剪切时,源行灰显(资源管理器同款视觉标记)。按路径集
  /// 下发三视图(cutPaths)。
  const cutPaths = useMemo(
    () =>
      clip != null && clip.cut
        ? new Set(clip.entries.map((e) => e.path))
        : new Set<string>(),
    [clip],
  );

  /// 目录三视图共享的行交互/状态 props(渲染序、排序、选择、门控进入、
  /// 重试、右键菜单)。searchActive:搜索词是否激活(空态文案随之区分
  /// 无匹配项/此文件夹为空)。onSortKey:details 表头点击以 headerClick
  /// 合成新 SortState,与工具栏排序菜单共享同一状态源。dragSourcePaths:
  /// 正被拖拽的行路径集(Task 9,拖拽成立时命中行挂 .drag-source)。
  const dirProps: DirViewProps = {
    rows,
    loading: browse.loading,
    listing: browse.listing,
    error: browse.error,
    selected: selection,
    cutPaths,
    rangeSet,
    dragSourcePaths,
    searchActive: searchQ !== "",
    sortState,
    onSortKey: (k) => setSortState(headerClick(sortState, k)),
    onRowClick: handleRowClick,
    // 双击钻取:目录位逐级;favdir 位(返工 §13.6)在命名空间内逐级
    // (openDir 按 nav.current.kind 分派,rel 由条目绝对路径反推)。
    onOpenDir: (e) =>
      openDir(
        nav.current.kind === "dir" || nav.current.kind === "favdir" ? nav.current.device : "",
        e.path,
      ),
    onClearSelect: handleClearSelect,
    onRetry: () => setReloadTick((x) => x + 1),
    onRowContextMenu: handleRowContextMenu,
    onBlankContextMenu: handleBlankContextMenu,
  };

  /// 右键菜单「打开」项呈现条件:目标为目录且当前为单选(spec §4.1)。
  /// fav 位同式(复合键选择集,size 判定同源)。
  const ctxCanOpen =
    ctxMenu?.target != null && ctxMenu.target.is_dir && selection.size === 1;
  /// 右键菜单「下载到本机…」项呈现条件:目录位/快捷命名空间位仅远端设备
  /// (本机文件本就在本机,download 拒自源;favdir 行恒属成员单设备);
  /// fav 位选择非空恒呈现(本机组经 downloadMixed 转 local_copy 落盘,
  /// 返工 §14.2;目录位本机选择本就无此入口)。
  const ctxCanDownload =
    ((nav.current.kind === "dir" || nav.current.kind === "favdir") &&
      selfDeviceId != null &&
      nav.current.device !== selfDeviceId) ||
    (nav.current.kind === "fav" && selection.size > 0);
  /// 「添加到快捷目录」项呈现条件(V3-3 T11,仅目录位):候选解析见
  /// favAddTargets——右键目标为目录且选择空/含该行,或多选非空且全为
  /// 目录;设备离线也可加(路径字符串,离线面由混合视图承担)。
  const ctxCanAddFav =
    nav.current.kind === "dir" &&
    favAddTargets(ctxMenu?.target ?? null, rows, selection) !== null;

  /// 视图宿主的拖放落点键(当前目录;根卷列表位不挂 = 不可落)。favdir
  /// 位(返工 §13.6)= 当前命名空间原生路径(joinRel)——粘贴/拖放落点
  /// 与 dir 位同形(device\0path),既有分派零改可落。
  const viewDropKey =
    nav.current.kind === "dir" && nav.current.path !== ""
      ? `${nav.current.device}\u0000${nav.current.path}`
      : nav.current.kind === "favdir"
        ? `${nav.current.device}\u0000${favdirPath}`
        : null;

  /// 状态栏速率汇总(Task 13):live 作业 Map 派生(不另拉 RPC;aggRate
  /// 仅 running 计速、非终态计数,与托盘 tooltip 同口径)。active=0 →
  /// null(状态栏不展示速率段)。
  const transferAgg = useMemo(() => aggRate(jobs.values()), [jobs]);

  /// 面包屑命名空间数据(AddressBar favCrumbs 单 prop;返工二 §14.3 扩位;
  /// §15.5 清单扩成员——编辑态解析的快捷命名空间地址表与下拉数据源同份;
  /// 返工裁定五 §17.1 扩 favs 着陆位):favdir/fav 位必传——清单 id+名+
  /// 成员集、当前快捷名(清单已无此 id(他端删除随 data_changed 同步)→
  /// 兜底 id 串,防御)、rel 段(pathSegments 分解;fav 位无 rel 段传空)。
  /// favs 位传清单 + inboxLoc(chip 下拉收件箱置顶项直达目标;self 设备
  /// 或 recv_dir 缺失 → null,下拉不列收件箱)。收件箱 dir 位(本机
  /// recv_dir)仅下发 inboxBase 比较基准(selfDeviceId 门在 App 侧——他端
  /// 目录即便路径雷同也不入命名空间;组件内 splitPath 段前缀匹配自推),
  /// 其余字段中性填空。其余位 undefined=旧形态(「Synshu 设备 ▸ 设备 ▸
  /// 路径」零回归)。
  let favCrumbs: {
    favorites: { id: string; name: string; members: { device: string; path: string }[] }[];
    currentFavId: string;
    currentFavName: string;
    relSegments: string[];
    inboxBase?: string;
    inboxLoc?: Loc | null;
  } | undefined;
  if (nav.current.kind === "favdir") {
    const fd = nav.current; // 提 const:回调内属性收窄失效(TS 纪律)
    favCrumbs = {
      favorites: favStore.state.doc.favorites.map((f) => ({
        id: f.id,
        name: f.name,
        members: f.members,
      })),
      currentFavId: fd.favId,
      currentFavName:
        favStore.state.doc.favorites.find((f) => f.id === fd.favId)?.name ?? fd.favId,
      relSegments: pathSegments(fd.rel),
    };
  } else if (nav.current.kind === "fav") {
    const fv = nav.current;
    favCrumbs = {
      favorites: favStore.state.doc.favorites.map((f) => ({
        id: f.id,
        name: f.name,
        members: f.members,
      })),
      currentFavId: fv.id,
      currentFavName:
        favStore.state.doc.favorites.find((f) => f.id === fv.id)?.name ?? fv.id,
      relSegments: [],
    };
  } else if (nav.current.kind === "favs") {
    // favs 着陆位(返工裁定五 §17.1):chip 下拉直达数据——清单 + 收件箱
    // 目标 Loc;无当前选中项(currentFavId/Name 中性空)。
    favCrumbs = {
      favorites: favStore.state.doc.favorites.map((f) => ({
        id: f.id,
        name: f.name,
        members: f.members,
      })),
      currentFavId: "",
      currentFavName: "",
      relSegments: [],
      inboxLoc:
        selfDeviceId != null && status?.recv_dir
          ? { kind: "dir", device: selfDeviceId, path: status.recv_dir }
          : null,
    };
  } else if (
    nav.current.kind === "dir" &&
    nav.current.device === selfDeviceId &&
    status?.recv_dir
  ) {
    favCrumbs = {
      favorites: [],
      currentFavId: "",
      currentFavName: "",
      relSegments: [],
      inboxBase: status.recv_dir,
    };
  }

  /// favdir 位他端删除收口(终审 Minor 2):当前命名空间 favId 在 store
  /// 清单已查无(他端删除随 data_changed 同步)→ 回设备视图,与 fav 位
  /// gone 屏同语义(fav 位走渲染分支,本效应仅 favdir 位生效,其余位早退)。
  useEffect(() => {
    if (nav.current.kind !== "favdir") return;
    const fd = nav.current; // 提 const:回调内属性收窄失效(TS 纪律)
    if (favStore.state.doc.favorites.some((f) => f.id === fd.favId)) return;
    navigate({ kind: "devices" });
  }, [nav.current, favStore.state.doc.favorites, navigate]);

  return (
    <div className={"app" + (dragging ? " dragging" : "")}>
      <Toolbar
        sortState={sortState}
        onSortState={setSortState}
        viewMode={viewMode}
        onViewMode={setViewMode}
        opsEnabled={
          (nav.current.kind === "dir" ||
            nav.current.kind === "fav" ||
            nav.current.kind === "favdir") &&
          selection.size > 0
        }
        pasteEnabled={canPaste}
        newEnabled={loggedIn && !favStore.state.readOnly}
        onNewFav={() => {
          // 新建下拉(返工 §13.2):清重命名态防残留,命名对话框恒新建态
          // (picker 内新建同款防御);提交经既有 handleFavCreate。
          setFavRenameId(null);
          setFavNameOpen(true);
        }}
        theme={theme}
        onCycleTheme={() => setTheme((th) => nextTheme(th))}
        onCut={() =>
          nav.current.kind === "fav" ? copyCutMixed(selection, true) : clipFromSelection(true)
        }
        onCopy={() =>
          nav.current.kind === "fav" ? copyCutMixed(selection, false) : clipFromSelection(false)
        }
        onPaste={() => (nav.current.kind === "fav" ? requestPasteFav() : requestPaste())}
        onDelete={() =>
          nav.current.kind === "fav" ? deleteMixed(selection) : requestDelete([...selection])
        }
      />
      <AddressBar
        current={nav.current}
        canBack={nav.back.length > 0}
        canForward={nav.fwd.length > 0}
        canUp={!sameLoc(parentLoc(nav.current), nav.current)}
        onBack={goBack}
        onForward={goForward}
        onUp={goUp}
        onNavigate={navigate}
        deviceName={deviceName}
        searchQ={searchQ}
        onSearchChange={handleSearch}
        dropHoverKey={dropHoverKey}
        devices={devices}
        onAddressSubmit={handleAddressSubmit}
        onCopyPath={handleCopyPath}
        modalBlocked={modalBusy({ ctxMenu, confirmDel, pendingConflict, updatePage: updatePageOpen, approvalModal: approvalReq, favName: favNameOpen, favDel, favPick: favPickFor, favMemberPick: memberPickFav, favMenu: menuFav, favManage: manageFav, favDelFor })}
        favCrumbs={favCrumbs}
      />
      <div className="body">
        <Sidebar
          devices={devices}
          selfDeviceId={selfDeviceId}
          current={nav.current}
          dropHoverKey={dropHoverKey}
          showFavorites={loggedIn}
          favorites={favStore.state.doc.favorites}
          inboxPath={status?.recv_dir ?? null}
          onOpenFav={(id) => navigate({ kind: "fav", id })}
          onOpenFavDir={openFavDirMember}
          onFavContextMenu={handleFavContextMenu}
          onNavigate={navigate}
        />
        <main className="main">
          {!loggedIn ? (
            <GuideView onLoggedIn={refresh} evicted={evicted} />
          ) : nav.current.kind === "devices" ? (
            <DeviceTable
              devices={devices}
              selfDeviceId={selfDeviceId}
              onOpenDevice={enterDevice}
            />
          ) : nav.current.kind === "favs" ? (
            /* 快捷目录着陆页(返工裁定五 §17.1):收件箱置顶行 + 全部快捷
                目录行(双击/回车进分组视图);组头行点击落此位 */
            <FavsLandingView
              favorites={favStore.state.doc.favorites}
              inboxAvailable={selfDeviceId != null && !!status?.recv_dir}
              onOpenInbox={openInbox}
              onOpenFav={(id) => navigate({ kind: "fav", id })}
            />
          ) : nav.current.kind === "fav" ? (
            currentFav == null ? (
              // 快捷目录已删除(他端删除随 data_changed 同步,原位悬空):
              // 提示 + 返回设备列表。
              <div className="fav-empty">
                <div>{t("favorites.gone")}</div>
                <button className="btn" onClick={() => navigate({ kind: "devices" })}>
                  {t("favorites.backToDevices")}
                </button>
              </div>
            ) : (
              /* 主视图主体(T9;§16.1 分类恒定):key=fav.id 导航换快捷即
                  重挂载(FavView 内部锚点复位、useFavListing 重建引擎,
                  §16.4 SWR 缓存种子瞬时呈现);选择集走 fav 位复合键,
                  T11 动作对等:行右键装配条目菜单、Del/Ctrl+X/C 与工具
                  栏路由到 mixed 处理器(粘贴 T12 保持禁用)。 */
              <FavView
                key={currentFav.id}
                fav={currentFav}
                devices={devices}
                viewMode={viewMode}
                sortState={sortState}
                reloadTick={reloadTick}
                selected={selection}
                onSelected={(next) => setSelection(new Set(next))}
                searchQ={searchQ}
                onOpenDir={openFavDir}
                onRowContextMenu={handleFavRowContextMenu}
                onRowsChange={handleFavRowsChange}
                onBlankContextMenu={handleBlankContextMenu}
                onRetry={() => setReloadTick((x) => x + 1)}
                onSortKey={(k) => setSortState(headerClick(sortState, k))}
                onCount={setFavRowCount}
                onRowPointerDown={onFavPointerDown}
                dragSourcePaths={dragSourcePaths}
                dropHoverKey={dropHoverKey}
              />
            )
          ) : (
            <>
              {browsingOffline && (
                <div className="offline-banner" role="status">
                  {t("files.offlineBanner")}
                </div>
              )}
              {notice != null && (
                <div
                  className={"op-notice" + (notice.tone === "info" ? " info" : "")}
                  role="status"
                >
                  {notice.text}
                </div>
              )}
              {/* 视图宿主:框选拖拽启动面 + 行测量根(data-row-path 查询)+
                  指针拖拽启动面 + 当前目录拖放落点(data-drop-loc;根卷列表位不挂) */}
              <div
                className={
                  "view-host" + (viewDropKey !== null && viewDropKey === dropHoverKey ? " drop-hover" : "")
                }
                ref={viewHostRef}
                onMouseDown={onViewMouseDown}
                onPointerDown={onViewPointerDown}
                data-drop-loc={viewDropKey ?? undefined}
              >
                {viewMode === "details" ? (
                  <FileDetails {...dirProps} />
                ) : viewMode === "list" ? (
                  <FileList {...dirProps} />
                ) : (
                  <FileTiles {...dirProps} />
                )}
              </div>
            </>
          )}
        </main>
      </div>
      {/* 传输抽屉(Task 9,spec §7):窗底常驻(状态栏之上),收起=细摘要
          条,展开=六列持久历史面;控制 RPC 经 jobControl(错误转 toast),
          reinitiate/clear 成功后种子重拉翻新行面 */}
      <TransferDrawer
        jobs={jobs}
        busyIds={busyJobs}
        onPause={(id) => jobControl(id, daemonTransferPause)}
        onResume={(id) => jobControl(id, daemonTransferResume)}
        onCancel={(id) => jobControl(id, daemonTransferCancel)}
        onReinitiate={reinitiateJob}
        onClear={clearJob}
      />
      <StatusBar
        status={status}
        deviceCount={devices.length}
        itemCount={
          nav.current.kind === "dir" || nav.current.kind === "favdir"
            ? rows.length
            : nav.current.kind === "fav"
              ? favRowCount
              : null
        }
        selectedCount={selection.size}
        loading={(nav.current.kind === "dir" || nav.current.kind === "favdir") && browse.loading}
        transfer={transferAgg.active > 0 ? transferAgg : null}
      />
      {/* 窗内通知回显(Task 13):event.notify 同文案 5s toast,不堆叠 */}
      <NotifyToasts toasts={toasts} onExpire={expireToast} />
      <ContextMenu
        menu={ctxMenu}
        mode={ctxMenu?.favCtx != null ? "fav" : "dir"}
        canOpen={ctxCanOpen}
        canPaste={canPaste}
        canDownload={ctxCanDownload}
        canAddFav={ctxCanAddFav}
        onClose={() => setCtxMenu(null)}
        onOpen={(entry) => {
          const fav = ctxMenu?.favCtx;
          if (fav != null) {
            // fav 位「打开」:按条目引用回查来源设备,钻取与双击同一管线
            // (openFavDir 内解析最深包含成员 → favdir 命名空间)。
            const row = fav.rows.find((r) => r.entry === entry);
            if (row != null) openFavDir(row.device, entry.path);
            return;
          }
          openDir(
            nav.current.kind === "dir" || nav.current.kind === "favdir" ? nav.current.device : "",
            entry.path,
          );
        }}
        onCut={() =>
          ctxMenu?.favCtx != null
            ? copyCutMixed(selection, true)
            : clipFromSelection(true)
        }
        onCopy={() =>
          ctxMenu?.favCtx != null
            ? copyCutMixed(selection, false)
            : clipFromSelection(false)
        }
        onPaste={() => (nav.current.kind === "fav" ? requestPasteFav() : requestPaste())}
        onDelete={() =>
          ctxMenu?.favCtx != null
            ? deleteMixed(selection)
            : requestDelete([...selection])
        }
        onDownloadToLocal={() =>
          ctxMenu?.favCtx != null
            ? downloadMixed(selection)
            : requestDownloadToLocal()
        }
        onAddFav={() => {
          if (nav.current.kind !== "dir") return;
          const paths = favAddTargets(ctxMenu?.target ?? null, rows, selection);
          if (paths == null) return;
          setFavPickFor({ device: nav.current.device, paths });
        }}
        onRefresh={() => setReloadTick((x) => x + 1)}
      />
      <ConfirmDialog
        open={confirmDel != null}
        title={t("files.confirmPermanentTitle")}
        body={t("files.confirmPermanentBody", { count: confirmDel?.paths.length ?? 0 })}
        confirmLabel={t("files.confirmPermanentOk")}
        onConfirm={confirmForceDelete}
        onCancel={() => setConfirmDel(null)}
      />
      {/* 快捷位多组删除确认梯(V3-3 T11):dir 位 confirmDel 零改,独立
          实例;文案同款(files.confirmPermanent*),count = need_force 组
          总项数,确认经 confirmFavForceDelete 逐组 force 重发 */}
      <ConfirmDialog
        open={favDel != null}
        title={t("files.confirmPermanentTitle")}
        body={t("files.confirmPermanentBody", {
          count: favDel?.groups.reduce((n, g) => n + g.paths.length, 0) ?? 0,
        })}
        confirmLabel={t("files.confirmPermanentOk")}
        onConfirm={confirmFavForceDelete}
        onCancel={() => setFavDel(null)}
      />
      {/* 「添加到快捷目录」picker(V3-3 T11):目录位候选 → 点选快捷落
          addMembers / 新建走命名对话框;置于 FavNameDialog 之前(命名
          对话框同 z-index 后到者在上,叠 picker) */}
      <FavPickerDialog
        open={favPickFor != null}
        favorites={favStore.state.doc.favorites}
        busy={favBusy}
        onPick={handleFavPick}
        onNew={() => {
          setFavRenameId(null); // picker 内新建恒为新建态(同新建下拉防御,§13.2)
          setFavNameOpen(true);
        }}
        onClose={() => {
          // 命名对话框叠在其上时不吃掉关闭(Escape 两层共听 document,
          // 先关上层;下层由下一次 Escape/遮罩点击自关)。
          if (!favNameOpen) setFavPickFor(null);
        }}
      />
      {/* 「存入哪个成员」选择层(V3-3 T12 拖放三路):多成员快捷的拖放
          (favMemberDropFor,带源组集)与粘贴(favMemberPasteFor)共用一个
          实例;fav 本体渲染期从 store 解析,他端删除后解析为 null 即卸载
          (占线自动解除);onPick 经 handleFavMemberPick 按各自管线落目标 */}
      {memberPickFav != null && (
        <FavMemberPicker
          open
          fav={memberPickFav}
          devices={devices}
          onPick={handleFavMemberPick}
          onClose={() => {
            setFavMemberDropFor(null);
            setFavMemberPasteFor(null);
          }}
        />
      )}
      {/* 新建/重命名快捷目录对话框(V3-3 Task 8/13):受控模态;新建态
          提交经 favStore.createFav(成功关模态+toast,失败保留可重试),
          重命名态(renameTarget 非空,自右键菜单进入)经 renameFav,
          initial 预填当前名。取消清双态(favRenameId 一并退) */}
      <FavNameDialog
        open={favNameOpen}
        initial={renameTarget?.name}
        busy={favBusy}
        onSubmit={(name) => (renameTarget != null ? handleFavRename(name) : handleFavCreate(name))}
        onCancel={() => {
          setFavNameOpen(false);
          setFavRenameId(null);
        }}
      />
      {/* 快捷目录管理屏(V3-3 Task 13):成员清单逐行移除 + 底部删除快捷;
          manageFav 从 store 现值解析,他端删除后 null 即卸载(占线自动
          解除);重命名不在本弹层(右键菜单 → FavNameDialog);busy 用
          store 引擎在途位(共享 busy,跨 op 互斥由 mutate 门保证) */}
      {manageFav != null && (
        <FavManageDialog
          open
          fav={manageFav}
          devices={devices}
          busy={favStore.state.busy}
          onRemoveMember={handleFavRemoveMember}
          onDelete={() => setFavDelFor(manageFav.id)}
          onClose={() => {
            // 删除确认叠在其上时不吃掉关闭(T19 修⑦:Escape 两层共听
            // document,先关上层;下层由下一次 Escape/遮罩点击自关——
            // FavPickerDialog-over-FavNameDialog 同款守卫)。
            if (favDelFor == null) setManageFavId(null);
          }}
        />
      )}
      {/* 「删除快捷目录」确认梯(V3-3 Task 13):右键菜单与管理屏底部
          删除钮共用一个实例;置于管理屏之后(同 z-index 后到者在上,叠
          管理屏)。删除=只解除链接,不触碰远端文件(spec §3.1);成功后
          正浏览该快捷则回设备视图(confirmFavDelete) */}
      <ConfirmDialog
        open={favDelFor != null}
        title={t("favorites.deleteConfirmTitle")}
        body={t("favorites.deleteConfirmBody", { name: favDelTarget?.name ?? "" })}
        confirmLabel={t("favorites.deleteConfirmOk")}
        onConfirm={confirmFavDelete}
        onCancel={() => setFavDelFor(null)}
      />
      {/* 侧栏快捷行右键菜单(V3-3 Task 13):轻量浮层,项集按 menuFav
          装配(打开/重命名/管理成员/删除快捷);他端删除后 menuFav 为
          null → 空清单,TreeNodeMenu 防御不渲染 */}
      {favMenu != null && (
        <TreeNodeMenu
          x={favMenu.x}
          y={favMenu.y}
          items={favMenuItems}
          onClose={() => setFavMenu(null)}
        />
      )}
      {/* 新终端审批弹窗(V3-1):event.approval 到达即弹,同意/拒绝经
          approval.respond(Bearer=当前会话);Esc/遮罩=关闭不应答 */}
      <ApprovalModal
        req={approvalReq}
        busy={approvalBusy}
        onRespond={respondApproval}
        onDismiss={() => setApprovalReq(null)}
      />
      <ConflictDialog
        open={pendingConflict != null}
        conflicts={pendingConflict?.conflicts ?? []}
        onResolve={(policy) => pendingConflict?.exec(policy)}
        onCancel={() => setPendingConflict(null)}
      />
      {/* 更新页覆盖层(Task 15):独立于登录态、不进 nav 栈;托盘拉起
          (单实例路由/冷启动标记)与 available toast「查看」开页 */}
      {updatePageOpen && (
        <UpdatePage
          snapshot={updateSnap}
          busy={updateBusy}
          onApply={handleUpdateApply}
          onCheck={handleUpdateCheck}
          onClose={() => setUpdatePageOpen(false)}
        />
      )}
      {bandRect != null && (
        <div
          className="rubber-band"
          style={{
            left: bandRect.left,
            top: bandRect.top,
            width: bandRect.width,
            height: bandRect.height,
          }}
        />
      )}
      {/* 拖拽 ghost 浮卡(Task 9,spec §9;存档 v2 特写③):fixed 定位随光标
          (+12px,+12px),位置由 onDragPointerMove 直接写 transform(内联初值
          取 ghostPosRef,防挂载首帧闪现 0,0);pointer-events:none 不参与
          elementFromPoint 命中,落点测试语义不变。内容(首项图标+名称,多选
          加「N 项」徽章)拖拽成立时一次定死 */}
      {dragGhost != null && (
        <div
          className="drag-ghost"
          ref={ghostRef}
          style={{
            transform: `translate(${ghostPosRef.current.x + 12}px, ${ghostPosRef.current.y + 12}px)`,
          }}
          aria-hidden="true"
        >
          <span className="drag-ghost-icon">
            <EntryIcon entry={dragGhost.entry} />
          </span>
          <span className="drag-ghost-name">{dragGhost.entry.name}</span>
          {dragGhost.count > 1 && (
            <span className="drag-ghost-badge">
              {t("files.dragCount", { count: dragGhost.count })}
            </span>
          )}
        </div>
      )}
    </div>
  );
}
