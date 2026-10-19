/// MTDSS 主窗口壳(M3b/M3c):工具栏(排序/查看/操作按钮)/ 地址栏
/// (导航+面包屑+搜索)/ 左侧树 / 设备列表或目录三视图(详细信息|列表|
/// 平铺)/ 未登录引导态 / 离线提示条 / 状态栏。
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
/// 操作面 II(Task 12):内部剪贴板(剪切/复制 → {device, entries, cut};
/// MTDSS 不写系统剪贴板,MTDSS→Explorer 粘贴为 V1 反目标)与粘贴分派
/// (pastePlan 四态:upload/download/remote_copy/device_copy/local_copy;
/// 空内部剪贴板 → 桥读系统 CF_HDROP → upload);冲突预扫(源条目 × 目标
/// list,目录恒合并)→ 四键冲突对话框(全部键只作用剩余未决项)→
/// transfer.start 携 skip/overwrite;「下载到本机…」= 目录选择器 →
/// download 作业;拖入 = Tauri onDragDropEvent(dragDropEnabled,外部文件
/// 落真实路径)→ upload;**窗口内拖拽与拖出共用一套指针拖拽**(见下)。
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
import FileList, { type DirViewProps } from "./components/FileList";
import FileTiles from "./components/FileTiles";
import GuideView from "./components/GuideView";
import type { ViewMode } from "./components/Toolbar";
import Sidebar from "./components/Sidebar";
import StatusBar from "./components/StatusBar";
import Toolbar from "./components/Toolbar";
import { baseName, dragPlan, joinDirName, pastePlan, type PastePlan } from "./lib/clipboard";
import { applyListing, canEnterDir, filterEntries, sortEntries, type FsEntry, type Listing } from "./lib/browse";
import { zhCmp } from "./lib/collate";
import { modalBusy } from "./lib/modality";
import {
  clipboardFiles,
  daemonCopy,
  daemonDelete,
  daemonDevices,
  daemonHello,
  daemonList,
  daemonMove,
  daemonStat,
  daemonStatus,
  daemonTransferStart,
  dropResolveAtCursor,
  friendlyError,
  listenConnected,
  listenDevices,
  listenLogin,
  listenOnline,
  pickFolder,
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
import { scanConflicts, type Conflict, type ConflictSource } from "./lib/conflict";
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
  // 目录列表排序方向(默认升序;工具栏「排序」菜单切换,仅内存态)。
  const [sortDir, setSortDir] = useState<"asc" | "desc">("asc");
  // 内容区视图模式(工具栏「查看」菜单切换,默认详细信息,仅内存态)。
  const [viewMode, setViewMode] = useState<ViewMode>("details");
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
  // 冲突对话框挂起批:预扫出的冲突行 + 决策落定后的执行闭包。
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

  const refresh = useCallback(() => {
    void loadAll(setHello, setStatus, setDevices);
  }, []);

  /// 清空浏览期瞬时态:选择、Shift 锚点与搜索词。导航换位/复位/搜索词
  /// 变更时调用(搜索词清空保证状态栏计数始终对应当前渲染列表)。
  const clearTransient = useCallback(() => {
    setSelection(new Set());
    setAnchorPath(null);
    setSearchQ("");
  }, []);

  /// 登出/断连复位:回设备视图、清空历史与浏览状态,并令在途列表响应
  /// 过期(seq 前移)。
  const resetToDevices = useCallback(() => {
    seqRef.current += 1;
    setNav({ back: [], fwd: [], current: { kind: "devices" } });
    setBrowse(BROWSE_IDLE);
    clearTransient();
  }, [clearTransient]);

  useEffect(() => {
    refresh();
    const unlisteners: Array<Promise<() => void>> = [
      // 连接建立(含冷启动拉起后的首次连上):重拉全量 + 回设备视图
      // (daemon 可能已换实例,历史浏览位不再可信)。
      listenConnected(() => {
        refresh();
        resetToDevices();
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
    ];
    return () => {
      void Promise.all(unlisteners).then((uns) => uns.forEach((u) => u()));
    };
  }, [refresh, resetToDevices]);

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
        setBrowse({ loading: false, listing: null, error: String(e) });
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
    () => (browse.listing == null ? [] : sortEntries(filterEntries(browse.listing.entries, searchQ), sortDir, zhCmp())),
    [browse.listing, searchQ, sortDir],
  );
  const orderedPaths = useMemo(() => rows.map((r) => r.path), [rows]);

  /// 行点击:修饰键派生模式后交 applyClickSelect 解析(选择集与锚点)。
  const handleRowClick = useCallback(
    (path: string, e: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }) => {
      const res = applyClickSelect(selection, orderedPaths, path, clickMode(e), anchorPath);
      setSelection(res.selection);
      setAnchorPath(res.anchor);
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
      if (modalBusy({ ctxMenu, confirmDel, pendingConflict })) return;
      if (nav.current.kind !== "dir") return;
      const paths = [...selection];
      if (paths.length === 0) return;
      e.preventDefault();
      requestDelete(paths);
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [selection, ctxMenu, confirmDel, pendingConflict, nav.current, requestDelete]);

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
  /// 作业卡);其余四形态 transfer.start 起作业(卡片面 Task 13,此处仅
  /// 「已开始」info 提示)。剪切粘贴成功后清内部剪贴板(资源管理器同款)。
  /// 目标为当前目录时顺带刷新列表(新条目尽快可见;作业进度归 Task 13)。
  const executePlan = useCallback(
    async (
      plan: PastePlan,
      policy: { skip: string[]; overwrite: string[] },
      targetDevice: string,
      targetDir: string,
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
          case "local_copy":
            await daemonTransferStart({
              kind: "local_copy", paths: plan.paths, dir: plan.dir, cut: plan.cut,
            } satisfies TransferStartParams);
            break;
          case "device_copy": {
            // 同机 ctrl 面:cut → move 对(rename 语义);copy → fs 自动改名。
            // 无策略参数(与 CLI 同机糖同口径),逐条失败表以提示呈现。
            const r = plan.cut
              ? await daemonMove(
                  plan.device,
                  plan.paths.map((p) => ({ from: p, to: joinDirName(plan.dir, baseName(p)) })),
                )
              : await daemonCopy(plan.device, plan.paths, plan.dir);
            if (r.failed.length > 0) {
              setNotice({
                text: t("transfer.deviceCopyFailed", { message: r.failed[0].message }),
                tone: "error",
              });
            }
            break;
          }
          case "none":
            return;
        }
        if (plan.kind !== "device_copy") {
          setNotice({ text: t("transfer.startedHint"), tone: "info" });
        }
        if (plan.cut) setClip(null); // 剪切粘贴已发起,源删除由 daemon 按 cut 收尾
        if (nav.current.kind === "dir" && nav.current.device === targetDevice && nav.current.path === targetDir) {
          setReloadTick((x) => x + 1);
        }
      } catch (e) {
        notifyError(e);
      }
    },
    [nav.current, notifyError, t],
  );

  /// 分派核心:有策略形态(upload/download/remote_copy)先预扫目标目录
  /// (daemonList;NOT_FOUND = 目标目录尚不存在 = 无冲突;其余失败 → 提示
  /// 并中止,不盲发);预扫命中 → 挂冲突对话框(四键裁决,目录恒合并已
  /// 在 scanConflicts 滤除),无冲突 → 直接执行。device_copy/local_copy 无
  /// 策略面(fs 层自动改名/rename 覆盖),不进对话框直接执行。
  const runTransfer = useCallback(
    async (
      plan: PastePlan,
      sources: ConflictSource[],
      targetDevice: string,
      targetDir: string,
    ): Promise<void> => {
      const hasPolicy =
        plan.kind === "upload" || plan.kind === "download" || plan.kind === "remote_copy";
      const policy = { skip: [] as string[], overwrite: [] as string[] };
      if (hasPolicy) {
        let entries: FsEntry[] = [];
        try {
          entries = (await daemonList(targetDevice, targetDir)).entries;
        } catch (e) {
          const s = String(e);
          if (!s.includes("NOT_FOUND")) {
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
          // NOT_FOUND = 目标目录不存在(接收端会建出)= 无冲突。
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
  const statLocalSources = useCallback(async (files: string[]): Promise<ConflictSource[]> => {
    if (selfDeviceId == null) return [];
    const out: ConflictSource[] = [];
    for (const f of files) {
      try {
        const e = await daemonStat(selfDeviceId, f);
        out.push({ name: e.name, is_dir: e.is_dir, size: e.size });
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
  /// 四态分派;空 → 桥读系统 CF_HDROP → upload(系统剪贴板恒视为复制)。
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
      if (modalBusy({ ctxMenu, confirmDel, pendingConflict })) return;
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
  }, [selection, ctxMenu, confirmDel, pendingConflict, nav.current, requestPaste, clipFromSelection]);

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
      if (modalBusy({ ctxMenu, confirmDel, pendingConflict })) return;
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
  }, [dirKey, browsingOffline, ctxMenu, confirmDel, pendingConflict, statLocalSources, runTransfer]);

  /// 拖出窗口松开:drop_resolve_at_cursor(Task 10)→ 桌面/资源管理器目录
  /// → download 直落该目录;unresolvable → 钉死提示「请拖到桌面或使用
  /// 下载到本机…」。本机行拖出无意义(download 拒自源),直接忽略。
  const resolveExternalDrop = useCallback(
    (device: string, entries: FsEntry[]) => {
      // 模态占线不受理(与拖入/粘贴同门;OS 级落点不受 DOM 遮罩拦截)。
      if (modalBusy({ ctxMenu, confirmDel, pendingConflict })) return;
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
    [ctxMenu, confirmDel, pendingConflict, selfDeviceId, runTransfer, t],
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
    document.removeEventListener("pointermove", onDragPointerMove);
    document.removeEventListener("pointerup", onDragPointerUp);
    document.removeEventListener("pointercancel", onDragPointerCancel);
    window.removeEventListener("blur", onDragBlurCancel);
  }

  /// 拖拽 move:越 4px 阈值才成立(不干扰点击/双击);成立即在 body 上
  /// setPointerCapture——Chromium 捕获语义保证指针拖出窗口后 move/up 仍
  /// 送达(WebView2 同引擎;窗口失焦兜底取消)。窗内时按 elementFromPoint
  /// 命中 [data-drop-loc](树节点/面包屑/当前目录宿主)驱动悬停高亮。
  function onDragPointerMove(ev: PointerEvent): void {
    const d = dragRef.current;
    if (d == null || ev.pointerId !== d.pointerId) return;
    if (!d.started) {
      if (Math.hypot(ev.clientX - d.startX, ev.clientY - d.startY) < 4) return;
      d.started = true;
      setDragging(true);
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

  /// 目录三视图共享的行交互/状态 props(渲染序、选择、门控进入、重试、
  /// 右键菜单)。searchActive:搜索词是否激活(空态文案随之区分
  /// 无匹配项/此文件夹为空)。
  const dirProps: DirViewProps = {
    rows,
    loading: browse.loading,
    listing: browse.listing,
    error: browse.error,
    selected: selection,
    cutPaths,
    searchActive: searchQ !== "",
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

  return (
    <div className={"app" + (dragging ? " dragging" : "")}>
      <Toolbar
        sortDir={sortDir}
        onSortDir={setSortDir}
        viewMode={viewMode}
        onViewMode={setViewMode}
        opsEnabled={nav.current.kind === "dir" && selection.size > 0}
        pasteEnabled={canPaste}
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
      <StatusBar
        status={status}
        deviceCount={devices.length}
        itemCount={nav.current.kind === "dir" ? rows.length : null}
        selectedCount={selection.size}
      />
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
    </div>
  );
}
