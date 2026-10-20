import { describe, expect, it } from "vitest";

import { friendlyError, friendlyFsError, RECYCLE_UNAVAILABLE_MARKER } from "./daemon";

describe("friendlyFsError(daemon 错误 → 友好文案信号)", () => {
  it("friendly_fs_error_maps_code3_and_recycle_marker", () => {
    // code 3 = 对端离线 → offline:i18n key(文案留在 zh.json,纯函数不翻译)
    expect(friendlyFsError(3, "对端离线: dev-1")).toEqual({
      kind: "offline",
      text: "files.errOffline",
    });
    // code 1 + 「回收站不可用」标记串 → need_force(调用方弹永久删除确认后
    // force 重发;标记串与 daemon 侧 FsError::RecycleUnavailable 文案前缀
    // 及 CLI 侧 need_force_confirm 同源)
    const msg = `${RECYCLE_UNAVAILABLE_MARKER}: 无可用回收区`;
    expect(friendlyFsError(1, msg)).toEqual({ kind: "need_force", text: msg });
    // 其余错误 → plain,原文透传
    expect(friendlyFsError(1, "权限不足: C:\\a")).toEqual({
      kind: "plain",
      text: "权限不足: C:\\a",
    });
  });

  it("friendlyError 解析桥接错误串(`code N: message`)", () => {
    expect(friendlyError("code 3: 对端离线: dev-1")).toEqual({
      kind: "offline",
      text: "files.errOffline",
    });
    expect(friendlyError(`code 1: ${RECYCLE_UNAVAILABLE_MARKER}: x`)).toEqual({
      kind: "need_force",
      text: `${RECYCLE_UNAVAILABLE_MARKER}: x`,
    });
    // 非「code N: 」形态(未知错误/非字符串)→ plain 原文
    expect(friendlyError("任意失败文本")).toEqual({ kind: "plain", text: "任意失败文本" });
    expect(friendlyError(undefined)).toEqual({ kind: "plain", text: "undefined" });
  });
});
