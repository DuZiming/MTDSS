/// daemon 客户端封装:Tauri command 调用与 daemon 事件订阅的统一入口。
///
/// Rust 侧(ipc_bridge.rs)暴露的 command:daemon_hello / status / devices /
/// list / stat / delete / fs_move / fs_copy / transfer_start / transfer_list /
/// transfer_pause / transfer_resume / transfer_cancel /
/// login_request_code / login_verify / logout,另有本机命令 clipboard_files
/// (CF_HDROP 直读)与 pick_folder(目录选择器);并把 daemon 事件转发为
/// 窗口事件:daemon://online、daemon://devices、daemon://send_log、
/// daemon://login、daemon://connected、daemon://transfer、daemon://notify。
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { FsEntry, Listing } from "./browse";
import type { DeviceInfo, LoginStatus } from "./derived";
import type { JobSnapUI, TransferFrame } from "./transfer";

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

// ---------- 传输/复制操作面(Task 12) ----------

/// `transfer.start` 四形态的判别联合(与 daemon api.rs 的 IPC 契约逐字
/// 对齐;skip/overwrite 为冲突裁决后的策略名单,device_copy 不走此面)。
export type TransferStartParams =
  | { kind: "upload"; files: string[]; to: string; dir: string; skip?: string[]; overwrite?: string[]; cut?: boolean }
  | { kind: "download"; device: string; paths: string[]; dir: string; skip?: string[]; overwrite?: string[]; cut?: boolean }
  | { kind: "local_copy"; paths: string[]; dir: string; cut?: boolean }
  | { kind: "remote_copy"; from: string; paths: string[]; to: string; dir: string; skip?: string[]; overwrite?: string[]; cut?: boolean };

/// `transfer_start {…}` → {job_id}(作业卡面 Task 13 渲染;本任务起作业)。
export async function daemonTransferStart(
  params: TransferStartParams,
): Promise<{ job_id: string }> {
  return invoke<{ job_id: string }>("transfer_start", { params });
}

// ---------- 传输作业面(Task 13:卡片/控制/事件) ----------

/// `transfer_list {}` → 作业全量快照(按 id 升序;终态作业保留为历史面,
/// 只增不减——卡片可见性由 UI 侧收敛,列表本身就是种子+对账面)。字段与
/// daemon job_json 逐字对齐:含 src/dst,error 恒存在(null = 无错)。
export async function daemonTransferList(): Promise<JobSnapUI[]> {
  const out = await invoke<{ jobs: JobSnapUI[] }>("transfer_list");
  return out.jobs;
}

/// `transfer.pause {id}`:暂停(作业缺失/已终态/remote_copy 无数据面 →
/// 业务错误,卡片侧以提示呈现)。
export async function daemonTransferPause(id: string): Promise<void> {
  await invoke("transfer_pause", { id });
}

/// `transfer.resume {id}`:恢复(错误面同 pause)。
export async function daemonTransferResume(id: string): Promise<void> {
  await invoke("transfer_resume", { id });
}

/// `transfer.cancel {id}`:取消(缺失/已终态 → 业务错误)。
export async function daemonTransferCancel(id: string): Promise<void> {
  await invoke("transfer_cancel", { id });
}

/// `fs_move {device, pairs}` → {moved,failed}(device_copy 剪切:同机 ctrl
/// move,rename 语义文件目标覆盖、目录撞名报错;不走传输作业面)。
export async function daemonMove(
  device: string,
  pairs: { from: string; to: string }[],
): Promise<{ moved: number; failed: DeleteFailure[] }> {
  return invoke<{ moved: number; failed: DeleteFailure[] }>("fs_move", {
    device,
    pairs,
  });
}

/// `fs_copy {device, paths, dir}` → {copied,failed}(device_copy 复制:同机
/// ctrl copy,目标同名 unique_target 自动改名;不走传输作业面)。
export async function daemonCopy(
  device: string,
  paths: string[],
  dir: string,
): Promise<{ copied: number; failed: DeleteFailure[] }> {
  return invoke<{ copied: number; failed: DeleteFailure[] }>("fs_copy", {
    device,
    paths,
    dir,
  });
}

/// `clipboard_files {}` → {files:[…]}:系统剪贴板 CF_HDROP 直读(Windows)。
/// 空内部剪贴板时的粘贴转读入口;空表 = 无文件剪贴板(no-op)。
export async function clipboardFiles(): Promise<string[]> {
  const out = await invoke<{ files: string[] }>("clipboard_files");
  return out.files;
}

/// `pick_folder {}` → 目录路径 | null(系统目录选择器;null = 用户取消)。
export async function pickFolder(): Promise<string | null> {
  const out = await invoke<{ dir: string | null }>("pick_folder");
  return out.dir;
}

/// `drop_resolve_at_cursor {}` → 拖出落点解析(Task 10 契约):
/// {"kind":"desktop"|"dir"|"unresolvable","path"?:string,"reason"?:string}。
export interface DropResolveResult {
  kind: "desktop" | "dir" | "unresolvable";
  path?: string;
  reason?: string;
}

export async function dropResolveAtCursor(): Promise<DropResolveResult> {
  return invoke<DropResolveResult>("drop_resolve_at_cursor");
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

/// 订阅传输作业事件(Task 13,参数 = [`TransferFrame`],event.transfer 透传;
/// 状态翻转即发,进度 ≥400ms 节流)。前端经 applyTransferEvents 收敛进
/// 作业 Map:乱序/重复/终态后迟到进度全部在 reducer 收口。
export function listenTransfer(
  handler: (frame: TransferFrame) => void,
): Promise<UnlistenFn> {
  return listen<TransferFrame>("daemon://transfer", (e) => handler(e.payload));
}

/// 订阅接收完成通知(Task 13,参数 {"title","body"};daemon 侧已自发 OS
/// 系统通知,此处仅转发同文案供 UI 做窗内瞬时回显,不重复弹系统通知)。
export interface NotifyPayload {
  title: string;
  body: string;
}

export function listenNotify(
  handler: (n: NotifyPayload) => void,
): Promise<UnlistenFn> {
  return listen<NotifyPayload>("daemon://notify", (e) => handler(e.payload));
}
