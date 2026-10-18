/// 设备列表派生工具:排序(本机置顶)与信令状态文案。
///
/// 排序语义(设计 §3):设备列表中本机永远置顶并标注「本机」,
/// 与其余设备完全同构;其余按 name 稳定升序(UTF-16 码元序,同名保持
/// 原始相对次序)——服务器快照序本就任意,名称序才有意义。

/// daemon `devices` RPC 中的单台设备字段(恰含这四项)。
export interface DeviceInfo {
  device_id: string;
  name: string | null;
  platform: string;
  online: boolean;
}

/// 登录/在线状态的最小切片(`status` RPC 的子集)。
export interface LoginStatus {
  logged_in: boolean;
  online: boolean;
}

/// 名称排序键:null(会话里无名字段,名字权威在服务器)排最前按空名处理。
function nameKey(d: DeviceInfo): string {
  return d.name ?? "";
}

/// 设备列表排序:本机置顶,其余按 name 稳定升序;返回新数组不改入参。
export function sortDevices(
  devices: DeviceInfo[],
  selfDeviceId: string | null,
): DeviceInfo[] {
  const self =
    selfDeviceId == null ? [] : devices.filter((d) => d.device_id === selfDeviceId);
  const others = devices.filter((d) => d.device_id !== selfDeviceId);
  // Array.prototype.sort 自 ES2019 起为稳定排序:同名设备保序。
  // 比较用 UTF-16 码元序而非 localeCompare:locale 结果随环境变(本机
  // Windows zh collation 按拼音,CI en-US 按码点),同一输入两种序,测试
  // 与多端表现皆不确定;码元序跨环境确定(ASCII 名即字典序)。拼音序
  // collation(显式 zh collator + 能力探测回退)留 M3b。
  others.sort((a, b) => (nameKey(a) < nameKey(b) ? -1 : nameKey(a) > nameKey(b) ? 1 : 0));
  return [...self, ...others];
}

/// 信令状态文案三态:未登录 / 离线 / 在线(状态栏专用)。
export function onlineText(status: LoginStatus): string {
  if (!status.logged_in) return "未登录";
  return status.online ? "在线" : "离线";
}
