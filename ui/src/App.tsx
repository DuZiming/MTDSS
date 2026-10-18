/// MTDSS 主窗口壳(M3a):工具栏占位 / 地址栏占位 / 左侧树 /
/// 设备列表或未登录引导态 / 状态栏。
///
/// 数据面:启动时拉 hello/status/devices;此后靠 daemon 事件
/// (daemon://online、daemon://devices,由 Rust 桥转发)保持新鲜。
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import AddressBar from "./components/AddressBar";
import DeviceTable from "./components/DeviceTable";
import GuideView from "./components/GuideView";
import Sidebar from "./components/Sidebar";
import StatusBar from "./components/StatusBar";
import Toolbar from "./components/Toolbar";
import {
  daemonDevices,
  daemonHello,
  daemonStatus,
  listenDevices,
  listenOnline,
  type HelloInfo,
  type StatusInfo,
} from "./lib/daemon";
import type { DeviceInfo } from "./lib/derived";
import "./App.css";

/// 初始快照:hello(版本/登录态)+ status(在线态)+ devices(目录)。
async function loadAll(
  setHello: (h: HelloInfo | null) => void,
  setStatus: (s: StatusInfo | null) => void,
  setDevices: (d: DeviceInfo[]) => void,
) {
  try {
    setHello(await daemonHello());
  } catch {
    setHello(null); // daemon 未就绪:引导态兜底,事件桥连上后由用户操作刷新
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

export default function App() {
  const { t } = useTranslation();
  const [hello, setHello] = useState<HelloInfo | null>(null);
  const [status, setStatus] = useState<StatusInfo | null>(null);
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  // M3a 仅一个内容视图:「MTDSS设备」= 设备列表(顶层)。
  const [devicesSelected, setDevicesSelected] = useState(true);

  const refresh = useCallback(() => {
    void loadAll(setHello, setStatus, setDevices);
  }, []);

  useEffect(() => {
    refresh();
    const unlisteners: Array<Promise<() => void>> = [
      // 信令在线状态:只翻 status.online,避免整包覆盖(其余字段未变)
      listenOnline((online) =>
        setStatus((s) => (s == null ? s : { ...s, online })),
      ),
      // 设备快照:全量替换
      listenDevices((list) => setDevices(list)),
    ];
    return () => {
      void Promise.all(unlisteners).then((uns) => uns.forEach((u) => u()));
    };
  }, [refresh]);

  const selfDeviceId = hello?.device?.device_id ?? status?.device?.device_id ?? null;
  const loggedIn = status?.logged_in ?? hello?.logged_in ?? false;

  return (
    <div className="app">
      <Toolbar />
      <AddressBar breadcrumb={t("sidebar.devices")} />
      <div className="body">
        <Sidebar
          devicesSelected={devicesSelected}
          onSelectDevices={() => setDevicesSelected(true)}
        />
        <main className="main">
          {!loggedIn ? (
            <GuideView onLoggedIn={refresh} />
          ) : (
            <DeviceTable devices={devices} selfDeviceId={selfDeviceId} />
          )}
        </main>
      </div>
      <StatusBar status={status} deviceCount={devices.length} />
    </div>
  );
}
