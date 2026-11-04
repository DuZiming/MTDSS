/// 登录面测试双层(V3-1 Task 10):
/// 1. invoke 契约(fix-wave Defect B):vi.mock 掉 @tauri-apps 运行时,
///    断言桥命令名与参数面逐字——login_request_code 的 ca 透传(daemon
///    侧持久化到待登录槽,verify 复用)与 pick_ca_cert 取消语义。
/// 2. 登录 UI 状态机纯函数(loginReducer/parseLoginStep/qrExpiredByTime):
///    全转移(form→qr→approval→evict→form)、denied/expired 文案态、
///    扫码过期重生成与取消路径、wire 投影判别(畸形拒绝)。
import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));

import {
  approvalRespond,
  loginCancel,
  loginEvict,
  loginQrBegin,
  loginQrStatus,
  loginRequestCode,
  loginStatus,
  loginSubmitCode,
  pickCaCert,
} from "./daemon";
import type { DeviceInfo } from "./derived";
import {
  loginInitial,
  loginReducer,
  parseLoginStep,
  qrExpiredByTime,
  type LoginUiState,
} from "./login";

describe("login invoke 契约(桥命令名与参数面逐字)", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockResolvedValue({});
  });

  it("loginRequestCode threads ca path through to the bridge", async () => {
    invokeMock.mockResolvedValue({ code: "123456" });
    await loginRequestCode("https://s:8787", "a@b.c", "C:\\certs\\ca.pem");
    expect(invokeMock).toHaveBeenCalledWith("login_request_code", {
      server: "https://s:8787",
      email: "a@b.c",
      ca: "C:\\certs\\ca.pem",
    });
  });

  it("loginRequestCode without ca sends explicit null(桥 Option → None)", async () => {
    invokeMock.mockResolvedValue({ code: "123456" });
    await loginRequestCode("https://s", "a@b.c");
    expect(invokeMock).toHaveBeenCalledWith("login_request_code", {
      server: "https://s",
      email: "a@b.c",
      ca: null,
    });
  });

  it("pickCaCert returns the picked certificate path", async () => {
    invokeMock.mockResolvedValue({ path: "C:\\certs\\root.crt" });
    await expect(pickCaCert()).resolves.toBe("C:\\certs\\root.crt");
    expect(invokeMock).toHaveBeenCalledWith("pick_ca_cert");
  });

  it("pickCaCert null = user cancel(不放大为错误)", async () => {
    invokeMock.mockResolvedValue({ path: null });
    await expect(pickCaCert()).resolves.toBeNull();
  });
});

describe("登录新面 invoke 契约(V3-1 Task 10:七命令名与参数面逐字)", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockResolvedValue({});
  });

  it("loginSubmitCode {session_id,code} 命令面逐字", async () => {
    invokeMock.mockResolvedValue({ status: "ok", device_id: "d", name: null });
    await loginSubmitCode("sess-a", "123456");
    expect(invokeMock).toHaveBeenCalledWith("login_submit_code", {
      sessionId: "sess-a",
      code: "123456",
    });
  });

  it("loginStatus {session_id} / loginCancel {} 命令面逐字", async () => {
    await loginStatus("sess-b");
    expect(invokeMock).toHaveBeenCalledWith("login_status", { sessionId: "sess-b" });
    await loginCancel();
    expect(invokeMock).toHaveBeenCalledWith("login_cancel");
  });

  it("loginEvict {enroll_token,device_id} 驼峰透传(桥转 snake)", async () => {
    invokeMock.mockResolvedValue({ status: "ok", device_id: "d", name: null });
    await loginEvict("enroll-1", "dev-2");
    expect(invokeMock).toHaveBeenCalledWith("login_evict", {
      enrollToken: "enroll-1",
      deviceId: "dev-2",
    });
  });

  it("loginQrBegin server/ca/name 三参透传(ca/name 缺省显式 null = 桥 Option → None)", async () => {
    invokeMock.mockResolvedValue({
      qr_url: "https://www.mtdss.top/qr?t=t",
      token: "t",
      expires_at: 1,
    });
    await loginQrBegin("https://s:8787", "C:\\ca.pem", "本机名");
    expect(invokeMock).toHaveBeenCalledWith("login_qr_begin", {
      server: "https://s:8787",
      ca: "C:\\ca.pem",
      name: "本机名",
    });
    await loginQrBegin("https://s:8787");
    expect(invokeMock).toHaveBeenCalledWith("login_qr_begin", {
      server: "https://s:8787",
      ca: null,
      name: null,
    });
  });

  it("loginQrStatus {token} / approvalRespond {session_id,allow} 命令面逐字", async () => {
    await loginQrStatus("qrt-1");
    expect(invokeMock).toHaveBeenCalledWith("login_qr_status", { token: "qrt-1" });
    await approvalRespond("sess-a", false);
    expect(invokeMock).toHaveBeenCalledWith("approval_respond", {
      sessionId: "sess-a",
      allow: false,
    });
  });
});

// ---------- 登录 UI 状态机(V3-1 Task 10,纯函数) ----------

/// 服务器 limit 清单两行(golden login.verify.resp.limit.json 同形)。
const devices: DeviceInfo[] = [
  { device_id: "dev-1", name: "契约本机", platform: "windows", online: true },
  { device_id: "dev-2", name: "契约对端", platform: "linux", online: false },
];

describe("loginReducer(登录面状态机:全转移与文案态)", () => {
  it("initial = form 无终态文案", () => {
    expect(loginInitial()).toEqual({ phase: "form", terminal: null });
  });

  it("form → qr:qrEnter 入扫码态(qr=null=生成中),qrBegin 填会话", () => {
    let s: LoginUiState = loginReducer(loginInitial(), { type: "qrEnter" });
    expect(s).toEqual({ phase: "qr", qr: null, expired: false });
    s = loginReducer(s, {
      type: "qrBegin",
      qrUrl: "https://www.mtdss.top/qr?t=qrt-1",
      token: "qrt-1",
      expiresAt: 1760000300,
    });
    expect(s).toEqual({
      phase: "qr",
      qr: { qrUrl: "https://www.mtdss.top/qr?t=qrt-1", token: "qrt-1", expiresAt: 1760000300 },
      expired: false,
    });
  });

  it("form → approval:approvalWait 携会话号与到期", () => {
    const s = loginReducer(loginInitial(), {
      type: "approvalWait",
      sessionId: "sess-a",
      expiresAt: 1760000200,
    });
    expect(s).toEqual({
      phase: "approval",
      wait: { sessionId: "sess-a", expiresAt: 1760000200 },
    });
  });

  it("form → evict:evictPick 携凭据与设备清单", () => {
    const s = loginReducer(loginInitial(), {
      type: "evictPick",
      enrollToken: "enroll-1",
      devices,
    });
    expect(s).toEqual({ phase: "evict", pick: { enrollToken: "enroll-1", devices } });
  });

  it("approval → evict / qr → evict:轮询 limit 转场(evict 屏数据整体换新)", () => {
    const approval = loginReducer(loginInitial(), {
      type: "approvalWait",
      sessionId: "sess-a",
      expiresAt: 1760000200,
    });
    expect(
      loginReducer(approval, { type: "evictPick", enrollToken: "enroll-2", devices }).phase,
    ).toBe("evict");
    const qr = loginReducer(loginInitial(), { type: "qrEnter" });
    const qrLive = loginReducer(qr, {
      type: "qrBegin",
      qrUrl: "u",
      token: "t",
      expiresAt: 1,
    });
    expect(
      loginReducer(qrLive, { type: "evictPick", enrollToken: "enroll-3", devices }),
    ).toEqual({ phase: "evict", pick: { enrollToken: "enroll-3", devices } });
  });

  it("evict → evict:evict 结果再撞 limit 时凭据与清单原位换新(不回表单)", () => {
    const evict = loginReducer(loginInitial(), {
      type: "evictPick",
      enrollToken: "enroll-1",
      devices: [devices[0]],
    });
    const next = loginReducer(evict, {
      type: "evictPick",
      enrollToken: "enroll-9",
      devices,
    });
    expect(next).toEqual({ phase: "evict", pick: { enrollToken: "enroll-9", devices } });
  });

  it("cancel:approval / evict / qr(含过期)→ form 清终态文案", () => {
    const from = (s: LoginUiState) => loginReducer(s, { type: "cancel" });
    expect(from(loginReducer(loginInitial(), { type: "approvalWait", sessionId: "s", expiresAt: 1 }))).toEqual({
      phase: "form",
      terminal: null,
    });
    expect(from(loginReducer(loginInitial(), { type: "evictPick", enrollToken: "e", devices }))).toEqual({
      phase: "form",
      terminal: null,
    });
    const qrExpired = loginReducer(
      loginReducer(loginInitial(), {
        type: "qrBegin",
        qrUrl: "u",
        token: "t",
        expiresAt: 1,
      }),
      { type: "qrExpired" },
    );
    expect(from(qrExpired)).toEqual({ phase: "form", terminal: null });
    // 终态文案同样被 cancel 清(retry 按钮与取消共用一路)
    const denied = loginReducer(loginInitial(), { type: "terminal", outcome: "denied" });
    expect(from(denied)).toEqual({ phase: "form", terminal: null });
  });

  it("terminal denied/expired:form / approval / qr → form 文案态(拒绝/超时各一词)", () => {
    expect(loginReducer(loginInitial(), { type: "terminal", outcome: "denied" })).toEqual({
      phase: "form",
      terminal: "denied",
    });
    const approval = loginReducer(loginInitial(), {
      type: "approvalWait",
      sessionId: "s",
      expiresAt: 1,
    });
    expect(loginReducer(approval, { type: "terminal", outcome: "expired" })).toEqual({
      phase: "form",
      terminal: "expired",
    });
    const qr = loginReducer(loginInitial(), {
      type: "qrBegin",
      qrUrl: "u",
      token: "t",
      expiresAt: 1,
    });
    expect(loginReducer(qr, { type: "terminal", outcome: "denied" })).toEqual({
      phase: "form",
      terminal: "denied",
    });
  });

  it("qrExpired:仅「扫码且已有会话」响应——清图留 token 置过期,他态不动", () => {
    const qr = loginReducer(loginInitial(), {
      type: "qrBegin",
      qrUrl: "u",
      token: "qrt-1",
      expiresAt: 1,
    });
    const expired = loginReducer(qr, { type: "qrExpired" });
    expect(expired).toEqual({
      phase: "qr",
      qr: { qrUrl: "", token: "qrt-1", expiresAt: 1 },
      expired: true,
    });
    // 生成中(qr=null)不响应(尚无图可清)
    const starting = loginReducer(loginInitial(), { type: "qrEnter" });
    expect(loginReducer(starting, { type: "qrExpired" })).toBe(starting);
    // form/approval/evict 均不响应(原引用原样返回)
    const form = loginInitial();
    expect(loginReducer(form, { type: "qrExpired" })).toBe(form);
    expect(
      loginReducer(loginReducer(loginInitial(), { type: "approvalWait", sessionId: "s", expiresAt: 1 }), {
        type: "qrExpired",
      }).phase,
    ).toBe("approval");
  });

  it("qrExpired 后 qrBegin 原位重生成(过期标志复位、会话换新)", () => {
    const expired = loginReducer(
      loginReducer(loginInitial(), {
        type: "qrBegin",
        qrUrl: "u",
        token: "qrt-1",
        expiresAt: 1,
      }),
      { type: "qrExpired" },
    );
    const regenerated = loginReducer(expired, {
      type: "qrBegin",
      qrUrl: "https://www.mtdss.top/qr?t=qrt-2",
      token: "qrt-2",
      expiresAt: 1760000400,
    });
    expect(regenerated).toEqual({
      phase: "qr",
      qr: { qrUrl: "https://www.mtdss.top/qr?t=qrt-2", token: "qrt-2", expiresAt: 1760000400 },
      expired: false,
    });
  });

  it("qrExpiredByTime:到期秒 == now 即过期(服务器 TTL 权威,本地仅兜底)", () => {
    const qr = { qrUrl: "u", token: "t", expiresAt: 1760000300 };
    expect(qrExpiredByTime(qr, 1760000299)).toBe(false);
    expect(qrExpiredByTime(qr, 1760000300)).toBe(true);
    expect(qrExpiredByTime(qr, 1760000301)).toBe(true);
  });
});

describe("parseLoginStep(daemon login 投影判别:畸形拒绝)", () => {
  it("六形态逐字判别(ok/approval/limit/pending/denied/expired)", () => {
    expect(parseLoginStep({ status: "ok", device_id: "d1", name: "契约本机" })).toEqual({
      status: "ok",
      device_id: "d1",
      name: "契约本机",
    });
    expect(parseLoginStep({ status: "approval", session_id: "s", expires_at: 1 })).toEqual({
      status: "approval",
      session_id: "s",
      expires_at: 1,
    });
    expect(parseLoginStep({ status: "limit", enroll_token: "e", devices })).toEqual({
      status: "limit",
      enroll_token: "e",
      devices,
    });
    expect(parseLoginStep({ status: "pending", expires_at: 5 })).toEqual({
      status: "pending",
      expires_at: 5,
    });
    expect(parseLoginStep({ status: "denied" })).toEqual({ status: "denied" });
    expect(parseLoginStep({ status: "expired" })).toEqual({ status: "expired" });
  });

  it("缺 status / 未知 status / 非对象 → null(判别失败,UI 报错误文案)", () => {
    expect(parseLoginStep({})).toBeNull();
    expect(parseLoginStep({ status: "unknown" })).toBeNull();
    expect(parseLoginStep("code 1: boom")).toBeNull();
    expect(parseLoginStep(null)).toBeNull();
  });

  it("形态缺必需字段 → null(approval 缺 session_id / limit 缺清单)", () => {
    expect(parseLoginStep({ status: "approval" })).toBeNull();
    expect(parseLoginStep({ status: "limit", enroll_token: "e" })).toBeNull();
    expect(
      parseLoginStep({ status: "limit", enroll_token: "e", devices: [{ nope: 1 }] }),
    ).toBeNull();
  });
});
