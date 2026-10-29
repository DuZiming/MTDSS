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
import ConflictDialog from "./components/ConflictDialog";
import ContextMenu, { type CtxMenuState } from "./components/ContextMenu";
import ConfirmDialog from "./components/ConfirmDialog";
import DeviceTable from "./components/DeviceTable";
import FileDetails from "./components/FileDetails";
import FileList, { EntryIcon, type DirViewProps } from "./components/FileList";
import FileTiles from "./components/FileTiles";
import GuideView from "./components/GuideView";
import type { ViewMode } from "./components/Toolbar";
import Sidebar from "./components/Sidebar";
import StatusBar from "./components/StatusBar";
import Toolbar from "./components/Toolbar";
import TransferDrawer from "./components/TransferDrawer";
import { NotifyToasts, type ToastItem } from "./components/NotifyToasts";
import UpdatePage from "./components/UpdatePage";
import { dragPlan, pastePlan, type PastePlan } from "./lib/clipboard";
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
  listenConnected,
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
  type DropResolveResult,
  type HelloInfo,
  type StatusInfo,
  type TransferStartParams,
} from "./lib/daemon";
import type { DeviceInfo } from "./lib/derived";
import {
  navBack,
  navForward,
  navGo,
  parentLoc,
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

/// 两位相等:设备层与设备层相等;目录位比较设备 + 路径。
function sameLoc(a: Loc, b: Loc): boolean {
  if (a.kind !== b.kind) return false;
  return a.kind !== "dir" || b.kind !== "dir" || (a.device === b.device && a.path === b.path);
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
  // 指针拖拽会话(ref 不触发渲染):越阈后进入;悬停高亮/拖拽中光标
  // 另用 state。outside = 指针当前在窗口外(拖出中)。
  const dragRef = useRef<{
    pointerId: number;
    startX: number;
    startY: number;
    device: string;
    srcDir: string;
    entries: FsEntry[];
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
  // 正被拖拽的行路径集(下发三视图挂 .drag-source 半透明;null = 无拖拽)。
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

  useEffect(() => {
    refresh();
    const unlisteners: Array<Promise<() => void>> = [
      // 连接建立(含冷启动拉起后的首次连上):重拉全量 + 回设备视图
      // (daemon 可能已换实例,历史浏览位不再可信)。
      listenConnected(() => {
        refresh();
        resetToDevices();
        seedTransfers();
      }),
      // 信令在线状态:只翻 status.online,避免整包覆盖(其余字段未变)
      listenOnline((online) =>
        setStatus((s) => (s == null ? s : { ...s, online })),
      ),
      // 设备快照:全量替换
      listenDevices((list) => setDevices(list)),
      // 登录态翻转(托盘/CLI 侧登录登出同样到达):全量刷新;登出回引导态
      listenLogin((loggedIn) => {
        refresh();
        if (!loggedIn) resetToDevices();
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
  }, [refresh, resetToDevices, seedTransfers, pushToast, applyUpdateFrame]);

  /// 窗口 focus 重拉 devices(Task 13 打磨):切走再切回时设备在线态/新
  /// 设备尽快对齐(事件只推增量翻转,focus 全量重拉是对账面);失败静默
  /// (事件流兜底)。hello/status 不重拉:登录/在线态均有事件翻转面。
  useEffect(() => {
    const onFocus = () => {
      void daemonDevices()
        .then((d) => setDevices(d.devices))
        .catch(() => {});
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, []);

  const selfDeviceId = hello?.device?.device_id ?? status?.device?.device_id ?? null;
  const loggedIn = status?.logged_in ?? hello?.logged_in ?? false;

  /// 目录列表拉取:当前 Loc 为目录位时发起;seqRef 单调递增 + cleanup
  /// 置 stale,双保险丢弃过期响应(快速连续导航不串台)。
  const dirKey =
    nav.current.kind === "dir"
      ? `${nav.current.device}\u0000${nav.current.path}`
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

  /// 地址栏编辑态提交(P4b Task 4):nav/deviceRoot → navigate(入导航
  /// 历史,解析规则全在 lib/address.ts 纯函数);error → toast 文案
  /// (deviceNotFound 携候选名插值;三键均阻断跳转)。空输入已在
  /// AddressBar 按取消处理,不会以 error 到达此处。
  const handleAddressSubmit = useCallback(
    (target: AddressTarget) => {
      switch (target.kind) {
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
  /// 防御(卷列表位无原生路径可复制)。
  const handleCopyPath = useCallback(() => {
    if (nav.current.kind !== "dir" || nav.current.path === "") return;
    const path = nav.current.path;
    void daemonCopyText(path)
      .then(() => pushToast({ title: t("address.pathCopied"), body: "", tone: "info" }))
      .catch((e: unknown) => {
        pushToast({ title: t("address.copyPath"), body: String(e), tone: "error" });
      });
  }, [nav.current, pushToast, t]);

  /// 当前浏览设备是否离线(以 devices 快照派生;树/主区门控同源)。
  /// 目录视图专用:设备层恒为 false。
  const browsingOffline = nav.current.kind === "dir" && !deviceOnline(nav.current.device);

  /// 双击目录行进入子目录(目标离线或正在浏览离线设备即拒绝,门控与树
  /// 一致;离线期双击为 no-op,顶部提示条已是可见解释)。
  const openDir = useCallback(
    (device: string, path: string) => {
      if (!canEnterDir(deviceOnline(device), browsingOffline)) return;
      navigate({ kind: "dir", device, path });
    },
    [deviceOnline, navigate, browsingOffline],
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
  const requestDelete = useCallback(
    (paths: string[]) => {
      if (deletingRef.current || paths.length === 0 || nav.current.kind !== "dir") return;
      const device = nav.current.device;
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

  /// Del 键删除:输入框聚焦不劫持;任一模态(右键菜单/删除确认/冲突
  /// 对话框,modalBusy)打开时不触发——评审 Important #1:曾漏挡
  /// pendingConflict,Del 会在冲突对话框遮罩下删掉当前选择;非空
  /// 选择且处于目录位才发起(离线设备照发,RPC code 3 走友好提示——
  /// 即「离线删除报友好文案」冒烟路径)。
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
      if (modalBusy({ ctxMenu, confirmDel, pendingConflict, updatePage: updatePageOpen })) return;
      if (nav.current.kind !== "dir") return;
      const paths = [...selection];
      if (paths.length === 0) return;
      e.preventDefault();
      requestDelete(paths);
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [selection, ctxMenu, confirmDel, pendingConflict, updatePageOpen, nav.current, requestDelete]);

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
        if (nav.current.kind === "dir" && nav.current.device === targetDevice && nav.current.path === targetDir) {
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
  const clipFromSelection = useCallback(
    (cut: boolean) => {
      if (nav.current.kind !== "dir" || selection.size === 0) return;
      const entries = rows.filter((r) => selection.has(r.path));
      if (entries.length === 0) return;
      setClip({ device: nav.current.device, entries, cut });
    },
    [nav.current, selection, rows],
  );

  /// 粘贴可用门控:目录位 + 非根卷列表(path="" 为卷列表,非文件夹) +
  /// 设备在线。可用性不看内部剪贴板是否为空——空则转读系统 CF_HDROP
  /// (可能恰有 Explorer 复制的文件)。
  const canPaste =
    nav.current.kind === "dir" && nav.current.path !== "" && !browsingOffline;

  /// 粘贴入口(工具栏/右键菜单/Ctrl+V 共用):内部剪贴板非空 → pastePlan
  /// 五态分派;空 → 桥读系统 CF_HDROP → upload(系统剪贴板恒视为复制)。
  /// 冲突对话框已开时不再受理(防连点叠出第二场预扫顶掉待决批)。
  const requestPaste = useCallback(() => {
    if (nav.current.kind !== "dir") return;
    if (pendingConflict != null) return;
    const { device, path } = nav.current;
    if (path === "") {
      setNotice({ text: t("files.cannotPasteRoot"), tone: "error" });
      return;
    }
    if (browsingOffline) {
      setNotice({ text: t("files.errOffline"), tone: "error" });
      return;
    }
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
  }, [nav.current, browsingOffline, pendingConflict, clip, selfDeviceId, runTransfer, statLocalSources, t]);

  /// 「下载到本机…」(右键菜单,仅远端设备条目——本机文件本就在本机):
  /// 目录选择器 → download 作业(冲突预扫对本机目录,daemonList(self)
  /// 短路本地 fs;NOT_FOUND = 空)。取消选择器即取消操作。
  const requestDownloadToLocal = useCallback(() => {
    if (nav.current.kind !== "dir" || selection.size === 0) return;
    if (selfDeviceId == null || nav.current.device === selfDeviceId) return;
    const entries = rows.filter((r) => selection.has(r.path));
    if (entries.length === 0) return;
    const device = nav.current.device;
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

  /// Ctrl+X / Ctrl+C / Ctrl+V:输入框聚焦不劫持;任一模态打开(modalBusy,
  /// 与 Del 键同一守卫)不触发;目录位才生效(V 与粘贴入口同一分派)。
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
      if (modalBusy({ ctxMenu, confirmDel, pendingConflict, updatePage: updatePageOpen })) return;
      if (nav.current.kind !== "dir") return;
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
  }, [selection, ctxMenu, confirmDel, pendingConflict, updatePageOpen, nav.current, requestPaste, clipFromSelection]);

  /// 拖入(外部文件 → 窗口):Tauri onDragDropEvent(dragDropEnabled=true
  /// 才有真实路径)。仅在目录位且非根卷列表且设备在线时受理;否则忽略
  /// (设备视图/根卷位拖入无目标)。冲突流与粘贴同一 runTransfer。
  /// 模态占线时同样不受理(评审补丁):冲突对话框开着时,Explorer 拖入
  /// 若直接 runTransfer 会顶掉待决批次、静默取消前一次粘贴。
  useEffect(() => {
    if (nav.current.kind !== "dir" || nav.current.path === "" || browsingOffline) return;
    const device = nav.current.device;
    const path = nav.current.path;
    const unlisten = getCurrentWindow().onDragDropEvent((ev) => {
      if (ev.payload.type !== "drop") return;
      if (modalBusy({ ctxMenu, confirmDel, pendingConflict, updatePage: updatePageOpen })) return;
      const files = ev.payload.paths;
      if (files.length === 0) return;
      void (async () => {
        const sources = await statLocalSources(files);
        void runTransfer(
          { kind: "upload", files, to: device, dir: path, cut: false },
          sources,
          device,
          path,
        );
      })();
    });
    return () => {
      void unlisten.then((u) => u());
    };
  }, [dirKey, browsingOffline, ctxMenu, confirmDel, pendingConflict, updatePageOpen, statLocalSources, runTransfer]);

  /// 拖出窗口松开:drop_resolve_at_cursor(Task 10)→ 桌面/资源管理器目录
  /// → download 直落该目录;unresolvable → 钉死提示「请拖到桌面或使用
  /// 下载到本机…」。本机行拖出无意义(download 拒自源),直接忽略。
  const resolveExternalDrop = useCallback(
    (device: string, entries: FsEntry[]) => {
      // 模态占线不受理(与拖入/粘贴同门;OS 级落点不受 DOM 遮罩拦截)。
      if (modalBusy({ ctxMenu, confirmDel, pendingConflict, updatePage: updatePageOpen })) return;
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
          setNotice({ text: t("transfer.dropUnresolvable"), tone: "error" });
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
    [ctxMenu, confirmDel, pendingConflict, updatePageOpen, selfDeviceId, runTransfer, t],
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
      // 期间不变,收尾时随 endDragSession 统一清。
      setDragGhost({ entry: d.entries[0], count: d.entries.length });
      setDragSourcePaths(new Set(d.entries.map((e) => e.path)));
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
  /// 命中 → dragPlan 分派(同设备=移动/跨设备=复制/任何→本机=落盘),
  /// 源目录即目标目录 = 原地 no-op;窗外松开 → 拖出解析。随后吞一次
  /// click(捕获相监听),防拖拽末尾 click 重选/清选。
  function onDragPointerUp(ev: PointerEvent): void {
    const d = dragRef.current;
    endDragSession();
    if (d == null || ev.pointerId !== d.pointerId) return;
    if (!d.started) return;
    dragMovedRef.current = true;
    if (d.outside) {
      resolveExternalDrop(d.device, d.entries);
      return;
    }
    const el = document.elementFromPoint(ev.clientX, ev.clientY)?.closest("[data-drop-loc]");
    const key = el?.getAttribute("data-drop-loc");
    if (key == null) return; // 无落点(拖到工具栏/侧栏空白等)= 取消
    const nul = key.indexOf("\u0000");
    const targetDevice = key.slice(0, nul);
    const targetDir = key.slice(nul + 1);
    // 原地放置(源目录即目标目录):同设备移动对自身 = no-op,静默。
    if (d.device === targetDevice && d.srcDir === targetDir) return;
    // 目标设备离线:提前友好提示(ctrl 面会失败,不如先说清)。
    if (!deviceOnline(targetDevice)) {
      setNotice({ text: t("files.errOffline"), tone: "error" });
      return;
    }
    const plan = dragPlan(
      d.device,
      d.entries.map((e) => e.path),
      selfDeviceId,
      targetDevice,
      targetDir,
    );
    if (plan.kind === "none") return;
    void runTransfer(plan, d.entries, targetDevice, targetDir);
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
      if (nav.current.kind !== "dir") return;
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
        device: nav.current.device,
        srcDir: nav.current.path,
        entries: selection.has(entry.path) ? rows.filter((r) => selection.has(r.path)) : [entry],
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
    onOpenDir: (e) => openDir(nav.current.kind === "dir" ? nav.current.device : "", e.path),
    onClearSelect: handleClearSelect,
    onRetry: () => setReloadTick((x) => x + 1),
    onRowContextMenu: handleRowContextMenu,
    onBlankContextMenu: handleBlankContextMenu,
  };

  /// 右键菜单「打开」项呈现条件:目标为目录且当前为单选(spec §4.1)。
  const ctxCanOpen =
    ctxMenu?.target != null && ctxMenu.target.is_dir && selection.size === 1;
  /// 右键菜单「下载到本机…」项呈现条件:当前浏览远端设备(本机文件
  /// 本就在本机,download 拒自源)。
  const ctxCanDownload =
    nav.current.kind === "dir" &&
    selfDeviceId != null &&
    nav.current.device !== selfDeviceId;

  /// 视图宿主的拖放落点键(当前目录;根卷列表位不挂 = 不可落)。
  const viewDropKey =
    nav.current.kind === "dir" && nav.current.path !== ""
      ? `${nav.current.device}\u0000${nav.current.path}`
      : null;

  /// 状态栏速率汇总(Task 13):live 作业 Map 派生(不另拉 RPC;aggRate
  /// 仅 running 计速、非终态计数,与托盘 tooltip 同口径)。active=0 →
  /// null(状态栏不展示速率段)。
  const transferAgg = useMemo(() => aggRate(jobs.values()), [jobs]);

  return (
    <div className={"app" + (dragging ? " dragging" : "")}>
      <Toolbar
        sortState={sortState}
        onSortState={setSortState}
        viewMode={viewMode}
        onViewMode={setViewMode}
        opsEnabled={nav.current.kind === "dir" && selection.size > 0}
        pasteEnabled={canPaste}
        theme={theme}
        onCycleTheme={() => setTheme((th) => nextTheme(th))}
        onCut={() => clipFromSelection(true)}
        onCopy={() => clipFromSelection(false)}
        onPaste={requestPaste}
        onDelete={() => requestDelete([...selection])}
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
        modalBlocked={modalBusy({ ctxMenu, confirmDel, pendingConflict, updatePage: updatePageOpen })}
      />
      <div className="body">
        <Sidebar
          devices={devices}
          selfDeviceId={selfDeviceId}
          current={nav.current}
          dropHoverKey={dropHoverKey}
          onNavigate={navigate}
        />
        <main className="main">
          {!loggedIn ? (
            <GuideView onLoggedIn={refresh} />
          ) : nav.current.kind === "devices" ? (
            <DeviceTable
              devices={devices}
              selfDeviceId={selfDeviceId}
              onOpenDevice={enterDevice}
            />
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
        itemCount={nav.current.kind === "dir" ? rows.length : null}
        selectedCount={selection.size}
        loading={nav.current.kind === "dir" && browse.loading}
        transfer={transferAgg.active > 0 ? transferAgg : null}
      />
      {/* 窗内通知回显(Task 13):event.notify 同文案 5s toast,不堆叠 */}
      <NotifyToasts toasts={toasts} onExpire={expireToast} />
      <ContextMenu
        menu={ctxMenu}
        canOpen={ctxCanOpen}
        canPaste={canPaste}
        canDownload={ctxCanDownload}
        onClose={() => setCtxMenu(null)}
        onOpen={(entry) =>
          openDir(nav.current.kind === "dir" ? nav.current.device : "", entry.path)
        }
        onCut={() => clipFromSelection(true)}
        onCopy={() => clipFromSelection(false)}
        onPaste={requestPaste}
        onDelete={() => requestDelete([...selection])}
        onDownloadToLocal={requestDownloadToLocal}
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
