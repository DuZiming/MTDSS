/// MTDSS 主窗口壳(M3b):工具栏占位 / 地址栏(导航+面包屑)/ 左侧树 /
/// 设备列表或目录详细信息视图 / 未登录引导态 / 状态栏。
///
/// 数据面:启动时拉 hello/status/devices;冷启动下这组初始拉取可能失败
/// (daemon 被桥拉起尚需 1-2s),故另订阅 daemon://connected——泵每次
/// (重)连成功都发,收到即重拉全量并回到设备视图。此后靠 daemon 事件
/// (daemon://online、daemon://devices、daemon://login)保持新鲜。
///
/// 导航面:NavState(Loc 历史栈)承载 后退/前进/上级;目录视图按当前
/// Loc 拉取列表,seqRef + applyListing 守卫丢弃过期响应;登出/断连全量
/// 复位(回到设备视图,清空浏览状态)。
import { useCallback, useEffect, useRef, useState } from "react";

import AddressBar from "./components/AddressBar";
import DeviceTable from "./components/DeviceTable";
import FileDetails from "./components/FileDetails";
import GuideView from "./components/GuideView";
import Sidebar from "./components/Sidebar";
import StatusBar from "./components/StatusBar";
import Toolbar from "./components/Toolbar";
import { applyListing, canEnterDir, type Listing } from "./lib/browse";
import {
  daemonDevices,
  daemonHello,
  daemonList,
  daemonStatus,
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
  const [hello, setHello] = useState<HelloInfo | null>(null);
  const [status, setStatus] = useState<StatusInfo | null>(null);
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  // 导航历史:current 初始为设备层(顶层视图)。
  const [nav, setNav] = useState<NavState>({
    back: [],
    fwd: [],
    current: { kind: "devices" },
  });
  // 目录列表排序方向(默认升序;排序切换菜单 Task 8 接线,本任务方向
  // 已在 FileDetails 生效)。
  const [sortDir] = useState<"asc" | "desc">("asc");
  // 当前行选择:路径集合(单选;Task 8 扩展 Ctrl/Shift 多选)。
  const [selection, setSelection] = useState<Set<string>>(new Set());
  // 当前目录列表(仅 kind==="dir" 时有意义)。
  const [browse, setBrowse] = useState<BrowseState>(BROWSE_IDLE);
  // 重试计数:变化即对当前目录重发列表请求。
  const [reloadTick, setReloadTick] = useState(0);
  // 列表响应序号:单调递增,过期响应经 applyListing 丢弃。
  const seqRef = useRef(0);

  const refresh = useCallback(() => {
    void loadAll(setHello, setStatus, setDevices);
  }, []);

  /// 登出/断连复位:回设备视图、清空历史与浏览状态,并令在途列表响应
  /// 过期(seq 前移)。
  const resetToDevices = useCallback(() => {
    seqRef.current += 1;
    setNav({ back: [], fwd: [], current: { kind: "devices" } });
    setBrowse(BROWSE_IDLE);
    setSelection(new Set());
  }, []);

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

  /// 导航:压栈前进(重复位亦入栈,历史保真)+ 清空选择。
  const navigate = useCallback((next: Loc) => {
    setNav((h) => navGo(h, next));
    setSelection(new Set());
  }, []);

  const goBack = useCallback(() => {
    setNav((h) => navBack(h) ?? h);
    setSelection(new Set());
  }, []);

  const goForward = useCallback(() => {
    setNav((h) => navForward(h) ?? h);
    setSelection(new Set());
  }, []);

  /// 上级:目录位逐级上溯,到根后即设备层;已在设备层则不动。
  const goUp = useCallback(() => {
    setNav((h) => {
      const up = parentLoc(h.current);
      return sameLoc(up, h.current) ? h : navGo(h, up);
    });
    setSelection(new Set());
  }, []);

  const deviceName = useCallback(
    (deviceId: string) => devices.find((d) => d.device_id === deviceId)?.name ?? deviceId,
    [devices],
  );

  const deviceOnline = useCallback(
    (deviceId: string) =>
      devices.find((d) => d.device_id === deviceId)?.online ?? false,
    [devices],
  );

  /// 双击目录行进入子目录(当前设备离线即拒绝,门控与树一致)。
  const openDir = useCallback(
    (device: string, path: string) => {
      if (!canEnterDir(deviceOnline(device), false)) return;
      navigate({ kind: "dir", device, path });
    },
    [deviceOnline, navigate],
  );

  /// 双击设备行进入该设备根(离线行禁入)。
  const enterDevice = useCallback(
    (d: DeviceInfo) => {
      if (!d.online) return;
      navigate({ kind: "dir", device: d.device_id, path: "" });
    },
    [navigate],
  );

  return (
    <div className="app">
      <Toolbar />
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
            <FileDetails
              loading={browse.loading}
              listing={browse.listing}
              error={browse.error}
              sortDir={sortDir}
              selected={selection}
              onSelect={(p) => setSelection(new Set([p]))}
              onOpenDir={(e) => openDir(nav.current.kind === "dir" ? nav.current.device : "", e.path)}
              onRetry={() => setReloadTick((x) => x + 1)}
            />
          )}
        </main>
      </div>
      <StatusBar
        status={status}
        deviceCount={devices.length}
        itemCount={nav.current.kind === "dir" ? (browse.listing?.entries.length ?? 0) : null}
        selectedCount={selection.size}
      />
    </div>
  );
}
