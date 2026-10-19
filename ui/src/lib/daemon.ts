/// daemon 客户端封装:Tauri command 调用与 daemon 事件订阅的统一入口。
///
/// Rust 侧(ipc_bridge.rs)暴露八个 command:
///   daemon_hello / status / devices / list / stat /
///   login_request_code / login_verify / logout
/// 并把 daemon 事件转发为窗口事件:daemon://online、daemon://devices、
/// daemon://send_log、daemon://login。
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { FsEntry, Listing } from "./browse";
import type { DeviceInfo, LoginStatus } from "./derived";

/// `hello` 返回:协议版本 + 登录态 + 本机设备(未登录为 null)。
export interface HelloInfo {
  version: string;
  logged_in: boolean;
  device: { device_id: string; name: string | null } | null;
}

/// `status` 返回:运行态快照。
export interface StatusInfo extends LoginStatus {
  server: string | null;
  device: { device_id: string; name: string | null } | null;
  recv_dir: string;
}

export async function daemonHello(): Promise<HelloInfo> {
  return invoke<HelloInfo>("daemon_hello");
}

export async function daemonStatus(): Promise<StatusInfo> {
  return invoke<StatusInfo>("status");
}

export async function daemonDevices(): Promise<{ devices: DeviceInfo[] }> {
  return invoke<{ devices: DeviceInfo[] }>("devices");
}

/// `list` 远程浏览:某设备某目录的条目切片(path 空串 = 根卷;字段名与
/// daemon 序列化逐字对齐,直接透传)。对端离线拒绝时抛 `code 3: …`,
/// 其余浏览失败抛 `code 1: …`(错误整形在 Rust 侧)。
export async function daemonList(device: string, path: string): Promise<Listing> {
  return invoke<Listing>("list", { device, path });
}

/// `stat` 远程浏览:单条目查询(错误映射同 daemonList)。
export async function daemonStat(device: string, path: string): Promise<FsEntry> {
  const out = await invoke<{ entry: FsEntry }>("stat", { device, path });
  return out.entry;
}

export async function loginRequestCode(
  server: string,
  email: string,
  ca?: string | null,
): Promise<{ code: string }> {
  return invoke<{ code: string }>("login_request_code", {
    server,
    email,
    ca: ca ?? null,
  });
}

export async function loginVerify(
  email: string,
  code: string,
  name?: string | null,
): Promise<{ device_id: string; name: string }> {
  return invoke<{ device_id: string; name: string }>("login_verify", {
    email,
    code,
    name: name ?? null,
  });
}

export async function logout(): Promise<void> {
  await invoke("logout");
}

/// 订阅连接建立事件(IPC 泵每次连上/重连上 daemon 都发)。
/// 前端收到后重拉 hello/status/devices——冷启动场景下挂载时的初始
/// 拉取会因 daemon 未就绪而失败,靠本信号补齐。
export function listenConnected(
  handler: () => void,
): Promise<UnlistenFn> {
  return listen("daemon://connected", () => handler());
}

/// 订阅信令在线状态事件(参数 {"online":bool})。
export function listenOnline(
  handler: (online: boolean) => void,
): Promise<UnlistenFn> {
  return listen<{ online: boolean }>("daemon://online", (e) =>
    handler(e.payload.online),
  );
}

/// 订阅设备快照事件(参数 {"devices":[…]},全量快照)。
export function listenDevices(
  handler: (devices: DeviceInfo[]) => void,
): Promise<UnlistenFn> {
  return listen<{ devices: DeviceInfo[] }>("daemon://devices", (e) =>
    handler(e.payload.devices),
  );
}

/// 订阅登录态翻转事件(参数 {"logged_in":bool};daemon 在登录/登出翻转
/// 成功后广播,前端据此全量刷新,登出即回引导态)。
export function listenLogin(
  handler: (loggedIn: boolean) => void,
): Promise<UnlistenFn> {
  return listen<{ logged_in: boolean }>("daemon://login", (e) =>
    handler(e.payload.logged_in),
  );
}
