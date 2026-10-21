/// daemon 客户端封装:Tauri command 调用与 daemon 事件订阅的统一入口。
///
/// Rust 侧(ipc_bridge.rs)暴露的 command:daemon_hello / status / devices /
/// list / stat / delete / fs_move / fs_copy / transfer_start / transfer_list /
/// transfer_pause / transfer_resume / transfer_cancel /
/// login_request_code / login_verify / logout,另有本机命令 clipboard_files
/// (CF_HDROP 直读)、pick_folder(目录选择器)与 pick_ca_cert(登录表单
/// 自签 CA 证书选择器);并把 daemon 事件转发为
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
/// daemon 序列化逐字对齐,直接透传)。对端离线拒绝时抛 `code 3: …`;
/// 其余浏览失败抛桥接错误串(形态与解析见 [`toRpcError`])。
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
/// 完全静默,spec §4.3),设施级回收不可用整体拒绝——错误报文附
/// `data.kind = RECYCLE_UNAVAILABLE`(code 1,见 [`RpcError`]),操作端
/// 确认后 force=true 整批重发;对端离线抛 code 3。响应 {deleted,recycled,failed}。
export async function daemonDelete(
  device: string,
  paths: string[],
  force: boolean,
): Promise<DeleteResult> {
  return invoke<DeleteResult>("delete", { device, paths, force });
}

/// daemon RPC 错误线格式(与 ipc 侧 `RpcError` 一字对齐):code/message
/// 恒在;`data` 仅 FsError 驱动的错误携带,形如 `{"kind": <FsError 稳定码>}`
/// (RECYCLE_UNAVAILABLE / NOT_FOUND / …)。桥接侧对无 data 错误保持
/// `code N: message` 人读串,对带 data 错误整体 JSON 序列化——两种形态
/// 都由 [`toRpcError`] 还原成本类型。
export interface RpcError {
  code: number;
  message: string;
  data?: { kind?: string };
}

/// `data.kind` 判定键(Rust `RpcError::kind` 的 TS 镜像,None-safe):
/// 无 data / data 非对象 / kind 非字符串 → undefined。
export function kind(e: RpcError): string | undefined {
  const d = e.data;
  if (d == null || typeof d !== "object") return undefined;
  const k = (d as { kind?: unknown }).kind;
  return typeof k === "string" ? k : undefined;
}

/// 任意抛出值 → [`RpcError`] 视图:桥接 JSON 文本形态(FsError 驱动,
/// `data` 在场)直接还原;`code N: message` 人读形态反解析(无 data,
/// 离线/超时/未连接等桥接本地错误仍是此形态);其余(非 code 形态串 /
/// 非字符串)按 code 0 plain 化。标记串子串匹配已废(M4a-7,同版本随
/// 发不留双轨)——「回收站不可用」等字样不再参与判定。
export function toRpcError(e: unknown): RpcError {
  if (typeof e === "string") {
    const t = e.trim();
    if (t.startsWith("{")) {
      try {
        const v = JSON.parse(t) as {
          code?: unknown;
          message?: unknown;
          data?: unknown;
        };
        if (typeof v.code === "number" && typeof v.message === "string") {
          const out: RpcError = { code: v.code, message: v.message };
          if (v.data != null && typeof v.data === "object") {
            out.data = v.data as { kind?: string };
          }
          return out;
        }
      } catch {
        // 非 JSON:落回人读串解析
      }
    }
    const m = /^code (-?\d+): ([\s\S]*)$/.exec(e);
    if (m != null) return { code: Number(m[1]), message: m[2] };
    return { code: 0, message: e };
  }
  return { code: 0, message: String(e) };
}

/// 友好错误信号:kind 供调用方分支,text 的含义随 kind 而定——
/// offline → i18n key(调用方 t() 后呈现;文案留在 zh.json,纯函数不做
/// 翻译);need_force → daemon 原始报文(确认框上下文);plain → 原始报文。
export interface FriendlyFsError {
  kind: "offline" | "need_force" | "plain";
  text: string;
}

/// daemon RPC 错误 → 友好文案信号(纯函数,类型化判定):code 3(对端
/// 离线)→ offline + i18n key;`data.kind = RECYCLE_UNAVAILABLE` →
/// need_force(确认升级信号;与 CLI 侧 need_force_confirm 同口径);
/// 其余 → plain 原文。
export function friendlyFsError(e: RpcError): FriendlyFsError {
  if (e.code === 3) return { kind: "offline", text: "files.errOffline" };
  if (kind(e) === "RECYCLE_UNAVAILABLE") {
    return { kind: "need_force", text: e.message };
  }
  return { kind: "plain", text: e.message };
}

/// 桥接错误面(未知异常/字符串)→ 友好文案信号:先经 [`toRpcError`]
/// 还原,再交 [`friendlyFsError`]。
export function friendlyError(e: unknown): FriendlyFsError {
  return friendlyFsError(toRpcError(e));
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

/// `pick_ca_cert {}` → 证书文件路径 | null(系统文件选择器,桥侧
/// tauri-plugin-dialog 包装;null = 用户取消)。登录表单自签 CA 面使用:
/// daemon 把 ca 持久化到待登录槽,verify 复用,故只在「获取验证码」前
/// 选择一次即可。
export async function pickCaCert(): Promise<string | null> {
  const out = await invoke<{ path: string | null }>("pick_ca_cert");
  return out.path;
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
/// 对齐;skip/overwrite 为冲突裁决后的策略名单,device_copy 不走此面。
/// local_copy 自 M4b 移交债①起带策略——判定键与 ctrl 面同规则(目标
/// 目录拼名/move 的 to 串,换算收口在 lib/paste),本机→本机粘贴经此面
/// 起作业卡)。
export type TransferStartParams =
  | { kind: "upload"; files: string[]; to: string; dir: string; skip?: string[]; overwrite?: string[]; cut?: boolean }
  | { kind: "download"; device: string; paths: string[]; dir: string; skip?: string[]; overwrite?: string[]; cut?: boolean }
  | { kind: "local_copy"; paths: string[]; dir: string; skip?: string[]; overwrite?: string[]; cut?: boolean }
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

/// ctrl copy/move 的可选冲突策略表(M4a-4;字段面与 transfer.start 策略
/// 同名同形):skip/overwrite 为**引擎判定键**名单(同机 copy = 目标目录
/// 形态拼条目名、move = pairs 的 `to` 原串;换算收口在 lib/paste 的
/// planSameDeviceOp),缺省/空表 = legacy(引擎自动改名安全网,与旧端
/// 互通口径一致)。
export interface CtrlPolicy {
  skip: string[];
  overwrite: string[];
}

/// `fs_move {device, pairs, skip?, overwrite?}` → {moved,failed,skipped}:
/// device_copy 剪切(同机 ctrl move,rename 语义;不走传输作业面)。策略
/// 表缺省 = legacy;skipped 为命中 skip 的判定键名单(daemon 回带,UI 暂
/// 不额外呈现——决策已在对话框落定,跳过项静默与传输面同口径)。
export async function daemonMove(
  device: string,
  pairs: { from: string; to: string }[],
  policy?: CtrlPolicy,
): Promise<{ moved: number; failed: DeleteFailure[]; skipped?: string[] }> {
  return invoke<{ moved: number; failed: DeleteFailure[]; skipped?: string[] }>(
    "fs_move",
    {
      device,
      pairs,
      skip: policy?.skip,
      overwrite: policy?.overwrite,
    },
  );
}

/// `fs_copy {device, paths, dir, skip?, overwrite?}` →
/// {copied,failed,skipped}:device_copy 复制(同机 ctrl copy;不走传输
/// 作业面)。策略面同 [`daemonMove`](缺省 = legacy 自动改名)。
export async function daemonCopy(
  device: string,
  paths: string[],
  dir: string,
  policy?: CtrlPolicy,
): Promise<{ copied: number; failed: DeleteFailure[]; skipped?: string[] }> {
  return invoke<{ copied: number; failed: DeleteFailure[]; skipped?: string[] }>(
    "fs_copy",
    {
      device,
      paths,
      dir,
      skip: policy?.skip,
      overwrite: policy?.overwrite,
    },
  );
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
