/// 登录 UI 状态机纯函数(V3-1 Task 10):四相位 form / qr / approval /
/// evict 的转移收口 + daemon login 投影(status 判别形态)的 UI 侧判别。
///
/// 与 daemon 契约(crates/ipc/contract golden,T8)逐字对齐:
/// - verify / submit_code / status / evict / qr_status 六 op 全部返回
///   `{status: "ok"|"approval"|"limit"|"pending"|"denied"|"expired", …}`
///   同族形态([`parseLoginStep`] 判别;畸形 → null,UI 报错误文案);
/// - 扫码过期 / 轮询 denied/expired 落 form 文案态([`loginReducer`]
///   `terminal` 动作),limit(verify 直达或轮询转场)落 evict 踢选屏。
///
/// 全部纯函数(不改入参、不触 IO、不读时钟):轮询节奏与本地到期兜底的
/// 取时在组件 effect 收口,本层只认动作与显式 now 入参([`qrExpiredByTime`])。
import type { DeviceInfo } from "./derived";

/// 登录面四相位:form=邮箱登录 tab;qr=扫码登录 tab(qr=null=二维码生成
/// 中);approval=等待审批屏(输码/轮询);evict=超限踢选屏。
export type LoginPhase = "form" | "qr" | "approval" | "evict";

/// 终态文案两词(拒绝/超时;form 顶部警示条呈现,zh 键 guide.terminal.*)。
export type LoginTerminal = "denied" | "expired";

/// 扫码会话(login.qr_begin 投影 `{qr_url,token,expires_at}` 驼峰化)。
/// 过期时 qrUrl 清空(不再渲染旧图),token 保留标识会话。
export interface QrInfo {
  qrUrl: string;
  token: string;
  expiresAt: number;
}

/// 审批等待会话(verify approval 投影 `{session_id,expires_at}`)。
export interface ApprovalWait {
  sessionId: string;
  expiresAt: number;
}

/// 踢选凭据与服务器清单(verify/status/qr_status 的 limit 投影;devices
/// 字段面与 DeviceInfo 同形,本机不在清单——清单即「可被移出的他端」)。
export interface EvictPick {
  enrollToken: string;
  devices: DeviceInfo[];
}

export type LoginUiState =
  | { phase: "form"; terminal: LoginTerminal | null }
  | { phase: "qr"; qr: QrInfo | null; expired: boolean }
  | { phase: "approval"; wait: ApprovalWait }
  | { phase: "evict"; pick: EvictPick };

/// 状态机动作(组件侧副作用——RPC/轮询/取消——按相位择机派发)。
export type LoginAction =
  /// 切到扫码 tab(尚无会话,QR 位呈生成中)
  | { type: "qrEnter" }
  /// qr_begin 成功(重生成同此动作,原位换新)
  | { type: "qrBegin"; qrUrl: string; token: string; expiresAt: number }
  /// 二维码过期(轮询 expired,或本地到期兜底;清图留会话)
  | { type: "qrExpired" }
  /// verify 落 approval:入等待审批屏
  | { type: "approvalWait"; sessionId: string; expiresAt: number }
  /// verify/轮询落 limit:入踢选屏(evict 结果再撞 limit 同此动作换新)
  | { type: "evictPick"; enrollToken: string; devices: DeviceInfo[] }
  /// denied/expired 终态:回 form 文案态
  | { type: "terminal"; outcome: LoginTerminal }
  /// 取消(任一中间态/终态文案 → form;login.cancel RPC 由组件伴发)
  | { type: "cancel" };

/// 初态:邮箱登录 tab,无终态文案。
export function loginInitial(): LoginUiState {
  return { phase: "form", terminal: null };
}

/// 登录面转移收口。越权动作(如非扫码态的 qrExpired)原样返回旧引用,
/// 组件 memo 面(以及「不响应」断言)零成本。
export function loginReducer(state: LoginUiState, action: LoginAction): LoginUiState {
  switch (action.type) {
    case "qrEnter":
      return { phase: "qr", qr: null, expired: false };
    case "qrBegin":
      return {
        phase: "qr",
        qr: { qrUrl: action.qrUrl, token: action.token, expiresAt: action.expiresAt },
        expired: false,
      };
    case "qrExpired":
      // 仅「扫码且已有会话」可过期:清图不再渲染旧码,token/expiresAt
      // 保留(「重新生成」原位 qrBegin 换新;此前的过期态兜不了重放)。
      return state.phase === "qr" && state.qr != null && !state.expired
        ? { ...state, expired: true, qr: { ...state.qr, qrUrl: "" } }
        : state;
    case "approvalWait":
      return {
        phase: "approval",
        wait: { sessionId: action.sessionId, expiresAt: action.expiresAt },
      };
    case "evictPick":
      return {
        phase: "evict",
        pick: { enrollToken: action.enrollToken, devices: action.devices },
      };
    case "terminal":
      return { phase: "form", terminal: action.outcome };
    case "cancel":
      return { phase: "form", terminal: null };
  }
}

/// 扫码本地到期兜底(纯函数):服务器 TTL(120s)是权威——正常路径轮询
/// 会收到 expired;本判定供轮询 tick 在无事件到期时提前收口(服务器不可
/// 达时 UI 不挂死在旧码上)。到期秒 == now 即视为过期。
export function qrExpiredByTime(qr: QrInfo, nowSec: number): boolean {
  return qr.expiresAt <= nowSec;
}

// ---------- daemon login 投影判别(wire 形态,与 step_json 逐字对齐) ----------

/// daemon `login.qr_begin` 投影(wire 蛇形原样;qr_url 由 daemon 拼
/// `https://www.mtdss.top/qr?t=<token>`,120s TTL)。
export interface QrBeginInfo {
  qr_url: string;
  token: string;
  expires_at: number;
}

/// `login.verify` / `login.submit_code` / `login.status` / `login.evict` /
/// `login.qr_status` 六 op 的响应族(status 判别面;daemon step_json 投影,
/// jwt/refresh 恒不出 IPC)。字段名保持 wire 蛇形(与 golden 一字对齐)。
export type LoginStepWire =
  | { status: "ok"; device_id: string; name: string | null }
  | { status: "pending"; expires_at?: number }
  | { status: "approval"; session_id: string; expires_at?: number }
  | { status: "limit"; enroll_token: string; devices: DeviceInfo[] }
  | { status: "denied" }
  | { status: "expired" };

/// limit 清单单行判别:device_id 为 evict 动作必需键;其余字段缺失按
/// 显示兜底(null 名/空平台/false 在线),不因装饰字段拒整单。
function parseDeviceBrief(v: unknown): DeviceInfo | null {
  if (v == null || typeof v !== "object") return null;
  const o = v as Record<string, unknown>;
  if (typeof o.device_id !== "string") return null;
  return {
    device_id: o.device_id,
    name: typeof o.name === "string" ? o.name : null,
    platform: typeof o.platform === "string" ? o.platform : "",
    online: o.online === true,
  };
}

/// 任意 daemon 响应 → [`LoginStepWire`]:status 键判别 + 各形态必需字段
/// 校验(approval 需 session_id;limit 需 enroll_token + 合法 devices 清
/// 单;其余形态只认 status)。判别失败(缺 status/未知值/非对象/字段畸
/// 形)→ null,调用方以错误文案呈现——绝不把畸形报文当合法中间态。
export function parseLoginStep(v: unknown): LoginStepWire | null {
  if (v == null || typeof v !== "object") return null;
  const o = v as Record<string, unknown>;
  switch (o.status) {
    case "ok":
      return {
        status: "ok",
        device_id: typeof o.device_id === "string" ? o.device_id : "",
        name: typeof o.name === "string" ? o.name : null,
      };
    case "pending": {
      const out: LoginStepWire = { status: "pending" };
      if (typeof o.expires_at === "number") out.expires_at = o.expires_at;
      return out;
    }
    case "approval": {
      if (typeof o.session_id !== "string") return null;
      const out: LoginStepWire = { status: "approval", session_id: o.session_id };
      if (typeof o.expires_at === "number") out.expires_at = o.expires_at;
      return out;
    }
    case "limit": {
      if (typeof o.enroll_token !== "string" || !Array.isArray(o.devices)) return null;
      const devices: DeviceInfo[] = [];
      for (const d of o.devices) {
        const brief = parseDeviceBrief(d);
        if (brief == null) return null;
        devices.push(brief);
      }
      return { status: "limit", enroll_token: o.enroll_token, devices };
    }
    case "denied":
      return { status: "denied" };
    case "expired":
      return { status: "expired" };
    default:
      return null;
  }
}
