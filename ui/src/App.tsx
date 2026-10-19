/// MTDSS 主窗口壳(M3b):工具栏(排序/查看菜单)/ 地址栏(导航+面包屑+
/// 搜索)/ 左侧树 / 设备列表或目录三视图(详细信息|列表|平铺)/ 未登录
/// 引导态 / 离线提示条 / 状态栏。
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
/// 操作面(Task 11):右键菜单(空白处=粘贴占位/刷新;条目上=打开/剪切/
/// 复制/粘贴占位/删除)、框选(空白 mousedown 拖出选框,rubberBandSelect
/// 按行包围盒相交解析,启动即清 Shift 锚点)、工具栏操作按钮按选中态
/// 启用、Del 键删除。删除 = 回收优先直接 RPC(spec §4.3,可回收静默);
/// 「回收站不可用」标记 → 永久删除确认 → force 整批重发;code 3 →
/// 「设备离线」提示;其余失败 → 瞬时提示条,成功(或部分成功)后刷新。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import AddressBar from "./components/AddressBar";
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
import { applyListing, canEnterDir, filterEntries, sortEntries, type FsEntry, type Listing } from "./lib/browse";
import { zhCmp } from "./lib/collate";
import {
  daemonDelete,
  daemonDevices,
  daemonHello,
  daemonList,
  daemonStatus,
  friendlyError,
  listenConnected,
  listenDevices,
  listenLogin,
  listenOnline,
  type HelloInfo,
  type StatusInfo,
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
  // 「回收站不可用 → 永久删除」确认梯的待确认批次(paths 原样整批重发)。
  const [confirmDel, setConfirmDel] = useState<{ paths: string[] } | null>(null);
  // 瞬时操作提示(删除失败/设备离线等;null = 无)。自动消隐。
  const [notice, setNotice] = useState<string | null>(null);
  // 三视图宿主(框选的行测量根与拖拽启动面)。
  const viewHostRef = useRef<HTMLDivElement>(null);
  // 删除在途防重入(Del 键连按/菜单与工具栏并发触发)。
  const deletingRef = useRef(false);
  // 本次框选拖拽已越阈标记:用于吞掉 mouseup 后落在空白处的 click 清空,
  // 避免框选结果被随后的空白点击清掉。
  const bandedRef = useRef(false);

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
  /// 刷新列表(部分失败如实呈现,已成功项生效)。
  const deletePaths = useCallback(
    async (paths: string[], force: boolean): Promise<void> => {
      if (nav.current.kind !== "dir") return;
      const device = nav.current.device;
      try {
        const r = await daemonDelete(device, paths, force);
        if (r.failed.length > 0) {
          setNotice(t("files.deleteFailed", { message: r.failed[0].message }));
        }
        setSelection(new Set());
        setAnchorPath(null);
        setReloadTick((x) => x + 1);
      } catch (err) {
        const fe = friendlyError(err);
        if (fe.kind === "need_force") {
          setConfirmDel({ paths });
        } else if (fe.kind === "offline") {
          setNotice(t(fe.text));
        } else {
          setNotice(fe.text);
        }
      }
    },
    [nav.current, t],
  );

  /// 删除入口(工具栏/右键菜单/Del 键共用):非空选择 + 在途防重入。
  const requestDelete = useCallback(
    (paths: string[]) => {
      if (deletingRef.current || paths.length === 0 || nav.current.kind !== "dir") return;
      deletingRef.current = true;
      void deletePaths(paths, false).finally(() => {
        deletingRef.current = false;
      });
    },
    [deletePaths, nav.current],
  );

  /// 确认梯终步:force=true 永久删除整批(与 CLI rm 确认梯同口径)。
  const confirmForceDelete = useCallback(() => {
    const paths = confirmDel?.paths ?? [];
    setConfirmDel(null);
    if (paths.length === 0) return;
    deletingRef.current = true;
    void deletePaths(paths, true).finally(() => {
      deletingRef.current = false;
    });
  }, [confirmDel, deletePaths]);

  /// 瞬时提示自动消隐(6s),换提示即重置计时。
  useEffect(() => {
    if (notice == null) return;
    const timer = window.setTimeout(() => setNotice(null), 6000);
    return () => window.clearTimeout(timer);
  }, [notice]);

  /// Del 键删除:输入框聚焦不劫持;菜单/确认框打开时不重复触发;非空
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
      if (ctxMenu != null || confirmDel != null) return;
      if (nav.current.kind !== "dir") return;
      const paths = [...selection];
      if (paths.length === 0) return;
      e.preventDefault();
      requestDelete(paths);
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [selection, ctxMenu, confirmDel, nav.current, requestDelete]);

  /// 目录三视图共享的行交互/状态 props(渲染序、选择、门控进入、重试、
  /// 右键菜单)。searchActive:搜索词是否激活(空态文案随之区分
  /// 无匹配项/此文件夹为空)。
  const dirProps: DirViewProps = {
    rows,
    loading: browse.loading,
    listing: browse.listing,
    error: browse.error,
    selected: selection,
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

  return (
    <div className="app">
      <Toolbar
        sortDir={sortDir}
        onSortDir={setSortDir}
        viewMode={viewMode}
        onViewMode={setViewMode}
        opsEnabled={nav.current.kind === "dir" && selection.size > 0}
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
      />
      <div className="body">
        <Sidebar
          devices={devices}
          selfDeviceId={selfDeviceId}
          current={nav.current}
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
                <div className="op-notice" role="status">
                  {notice}
                </div>
              )}
              {/* 视图宿主:框选拖拽启动面 + 行测量根(data-row-path 查询) */}
              <div className="view-host" ref={viewHostRef} onMouseDown={onViewMouseDown}>
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
        onClose={() => setCtxMenu(null)}
        onOpen={(entry) =>
          openDir(nav.current.kind === "dir" ? nav.current.device : "", entry.path)
        }
        onDelete={() => requestDelete([...selection])}
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
