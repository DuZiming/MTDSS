/// daemon 客户端封装:Tauri command 调用与 daemon 事件订阅的统一入口。
///
/// Rust 侧(ipc_bridge.rs)暴露九个 command:
///   daemon_hello / status / devices / list / stat / delete /
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

// ---------- 同机操作面:删除(Task 11) ----------

/// `delete` 批内单条失败(字段与 daemon 序列化逐字对齐;code 为 FsError
/// 稳定错误码串,如 NOT_FOUND / PERMISSION_DENIED / INTERNAL)。
export interface DeleteFailure {
  path: string;
  code: string;
  message: string;
}

/// `delete` 响应:成功数 / 是否走了回收 / 逐条失败表(批不半途弃废)。
export interface DeleteResult {
  deleted: number;
  recycled: boolean;
  failed: DeleteFailure[];
}

/// `delete {device,paths,force}`:回收优先(force=false;可回收成功两端
/// 完全静默,spec §4.3),设施级回收不可用整体拒绝——错误报文含
/// [`RECYCLE_UNAVAILABLE_MARKER`](code 1),操作端确认后 force=true 整批
/// 重发;对端离线抛 code 3。响应 {deleted,recycled,failed}。
export async function daemonDelete(
  device: string,
  paths: string[],
  force: boolean,
): Promise<DeleteResult> {
  return invoke<DeleteResult>("delete", { device, paths, force });
}

/// 永久删除升级判定标记串:daemon 侧 `FsError::RecycleUnavailable` 的
/// thiserror 文案前缀,经 ctrl 原样透传到 RPC 报文(与 CLI 侧
/// need_force_confirm 同源一字不差)。UI 据此把「回收优先删除」升级为
/// 「确认后 force 永久删」。
export const RECYCLE_UNAVAILABLE_MARKER = "回收站不可用";

/// 友好错误信号:kind 供调用方分支,text 的含义随 kind 而定——
/// offline → i18n key(调用方 t() 后呈现;文案留在 zh.json,纯函数不做
/// 翻译);need_force → daemon 原始报文(确认框上下文);plain → 原始报文。
export interface FriendlyFsError {
  kind: "offline" | "need_force" | "plain";
  text: string;
}

/// daemon RPC 错误 → 友好文案信号(纯函数):code 3(对端离线)→ offline
/// + i18n key;报文含「回收站不可用」标记 → need_force(确认升级信号);
/// 其余 → plain 原文。与 CLI 侧 need_force_confirm 判定同口径。
export function friendlyFsError(code: number, message: string): FriendlyFsError {
  if (code === 3) return { kind: "offline", text: "files.errOffline" };
  if (message.includes(RECYCLE_UNAVAILABLE_MARKER)) {
    return { kind: "need_force", text: message };
  }
  return { kind: "plain", text: message };
}

/// 桥接错误面(未知异常/字符串)→ 友好文案信号:桥接侧把 RpcError 整形为
/// `code N: message` 字符串(ipc_bridge.rs rpc_err),此处反解析后交
/// [`friendlyFsError`];不匹配该形态的输入按 plain 原文处理。
export function friendlyError(e: unknown): FriendlyFsError {
  const s = typeof e === "string" ? e : String(e);
  const m = /^code (-?\d+): ([\s\S]*)$/.exec(s);
  if (m == null) return { kind: "plain", text: s };
  return friendlyFsError(Number(m[1]), m[2]);
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
