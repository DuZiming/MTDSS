import { describe, expect, it } from "vitest";

import { friendlyError, friendlyFsError, kind, toRpcError, type RpcError } from "./daemon";

/// 便捷构造:RPC 错误(data 缺省 = 无附加数据,与线格式缺省省略一致)。
function rpcErr(code: number, message: string, data?: { kind?: string }): RpcError {
  return data === undefined ? { code, message } : { code, message, data };
}

describe("kind(data.kind 判定键;Rust RpcError::kind 的 TS 镜像)", () => {
  it("data.kind 在场 → 原样返回", () => {
    expect(kind(rpcErr(1, "远端: …", { kind: "RECYCLE_UNAVAILABLE" }))).toBe(
      "RECYCLE_UNAVAILABLE",
    );
    expect(kind(rpcErr(1, "远端: …", { kind: "NOT_FOUND" }))).toBe("NOT_FOUND");
  });

  it("None-safe:无 data / data 非对象 / 缺 kind / kind 非字符串 → undefined", () => {
    // 无 data(旧 daemon / 桥接本地错误)
    expect(kind(rpcErr(1, "无附加数据"))).toBeUndefined();
    expect(
      kind({ code: 1, message: "m", data: null as unknown as { kind?: string } }),
    ).toBeUndefined();
    expect(kind({ code: 1, message: "m", data: "RECYCLE_UNAVAILABLE" as unknown as { kind?: string } })).toBeUndefined();
    expect(kind(rpcErr(1, "m", {}))).toBeUndefined();
    expect(kind({ code: 1, message: "m", data: { kind: 42 as unknown as string } })).toBeUndefined();
  });
});

describe("toRpcError(桥接错误串 → RpcError 视图)", () => {
  it("JSON 文本形态(FsError 驱动,data 在场)整体还原", () => {
    expect(
      toRpcError(
        '{"code":1,"message":"远端: RECYCLE_UNAVAILABLE 回收站不可用: x","data":{"kind":"RECYCLE_UNAVAILABLE"}}',
      ),
    ).toEqual({
      code: 1,
      message: "远端: RECYCLE_UNAVAILABLE 回收站不可用: x",
      data: { kind: "RECYCLE_UNAVAILABLE" },
    });
  });

  it("`code N: message` 人读形态反解析(无 data;离线/超时/未连接仍走此形态)", () => {
    expect(toRpcError("code 3: 对端离线: dev-1")).toEqual({
      code: 3,
      message: "对端离线: dev-1",
    });
    expect(toRpcError("code -32000: RPC 调用超时(10s): list")).toEqual({
      code: -32000,
      message: "RPC 调用超时(10s): list",
    });
  });

  it("其余(非 code 形态串 / 非字符串)→ code 0 plain 化", () => {
    expect(toRpcError("任意失败文本")).toEqual({ code: 0, message: "任意失败文本" });
    expect(toRpcError(undefined)).toEqual({ code: 0, message: "undefined" });
    expect(toRpcError(42)).toEqual({ code: 0, message: "42" });
    // 形似 JSON 但非 RpcError 形状(code 非数值)→ 不误认,按原文 plain
    expect(toRpcError('{"code":"1","message":"m"}')).toEqual({
      code: 0,
      message: '{"code":"1","message":"m"}',
    });
  });
});

describe("friendlyFsError(daemon RPC 错误 → 友好文案信号,类型化判定)", () => {
  it("code 3 → offline:i18n key(文案留在 zh.json,纯函数不翻译)", () => {
    expect(friendlyFsError(rpcErr(3, "对端离线: dev-1"))).toEqual({
      kind: "offline",
      text: "files.errOffline",
    });
  });

  it("data.kind = RECYCLE_UNAVAILABLE → need_force(确认升级信号;与 CLI need_force_confirm 同口径)", () => {
    const e = rpcErr(1, "远端: RECYCLE_UNAVAILABLE 回收站不可用: 无可用回收区", {
      kind: "RECYCLE_UNAVAILABLE",
    });
    expect(friendlyFsError(e)).toEqual({ kind: "need_force", text: e.message });
  });

  it("标记串判定已废:message 含「回收站不可用」字样但无 data.kind → plain(旧 daemon 同此)", () => {
    expect(friendlyFsError(rpcErr(1, "回收站不可用: 无可用回收区"))).toEqual({
      kind: "plain",
      text: "回收站不可用: 无可用回收区",
    });
    expect(friendlyFsError(rpcErr(1, "远端: RECYCLE_UNAVAILABLE 回收站不可用: x"))).toEqual({
      kind: "plain",
      text: "远端: RECYCLE_UNAVAILABLE 回收站不可用: x",
    });
  });

  it("其余错误 → plain 原文透传", () => {
    expect(friendlyFsError(rpcErr(1, "权限不足: C:\\a"))).toEqual({
      kind: "plain",
      text: "权限不足: C:\\a",
    });
  });
});

describe("friendlyError(任意抛出值 → 友好文案信号)", () => {
  it("JSON 串端到端:data.kind 命中 need_force", () => {
    expect(
      friendlyError(
        '{"code":1,"message":"远端: RECYCLE_UNAVAILABLE 回收站不可用: x","data":{"kind":"RECYCLE_UNAVAILABLE"}}',
      ).kind,
    ).toBe("need_force");
  });

  it("人读串:code 3 → offline;标记串已废 → plain;未知形态 → plain", () => {
    expect(friendlyError("code 3: 对端离线: dev-1")).toEqual({
      kind: "offline",
      text: "files.errOffline",
    });
    expect(friendlyError("code 1: 回收站不可用: x")).toEqual({
      kind: "plain",
      text: "回收站不可用: x",
    });
    expect(friendlyError("任意失败文本")).toEqual({ kind: "plain", text: "任意失败文本" });
  });
});
