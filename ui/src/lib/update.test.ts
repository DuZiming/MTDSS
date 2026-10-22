import { describe, expect, it } from "vitest";

import {
  canInstall,
  formatProgress,
  updateStateFromEvents,
  updateTierLabel,
  type UpdateEvent,
  type UpdateSnapshot,
} from "./update";

/// 合成一条线上事件帧(event.update 形;与 daemon state_json 字段逐字对齐,
/// 缺省字段 = 线上省略形态)。
function ev(over: Partial<UpdateEvent> & { state: UpdateEvent["state"] }): UpdateEvent {
  return {
    current: "1.0.0",
    ...over,
  };
}

describe("updateStateFromEvents(事件收敛 reducer)", () => {
  it("full_transition_table_idle_to_installing", () => {
    // 全迁移表:idle → available → downloading(节流进度逐步展示)→
    // ready → installing,每帧完整采纳(daemon 状态机正传)。
    let s: UpdateSnapshot | null = null;
    s = updateStateFromEvents(s, ev({ state: "idle" }));
    expect(s!.state).toBe("idle");
    expect(s!.current).toBe("1.0.0");

    s = updateStateFromEvents(s, ev({ state: "available", latest: "1.0.1", notes: "修复若干问题" }));
    expect(s!.state).toBe("available");
    expect(s!.latest).toBe("1.0.1");
    expect(s!.notes).toBe("修复若干问题");
    expect(s!.progress).toBeNull();

    s = updateStateFromEvents(s, ev({ state: "downloading", latest: "1.0.1" }));
    expect(s!.state).toBe("downloading");
    expect(s!.progress).toBeNull(); // 翻转帧不伪造进度

    s = updateStateFromEvents(s, ev({ state: "downloading", latest: "1.0.1", progress: 0.4 }));
    s = updateStateFromEvents(s, ev({ state: "downloading", latest: "1.0.1", progress: 0.9 }));
    expect(s!.state).toBe("downloading");
    expect(s!.progress).toBe(0.9); // 节流进度帧逐步展示

    s = updateStateFromEvents(s, ev({ state: "ready", latest: "1.0.1" }));
    expect(s!.state).toBe("ready");
    expect(s!.progress).toBeNull();

    s = updateStateFromEvents(s, ev({ state: "installing", latest: "1.0.1" }));
    expect(s!.state).toBe("installing");

    // 纯函数:入参 prev 不被改写
    expect(s).not.toBeNull();
  });

  it("idempotent_same_frame_returns_prev_reference", () => {
    // 幂等:同帧重复投递(事件重放/事件与 RPC 返回同形同值)→ 返回
    // 原引用,React 侧零重渲染。
    const first = updateStateFromEvents(null, ev({ state: "available", latest: "1.0.1" }));
    const again = updateStateFromEvents(first, ev({ state: "available", latest: "1.0.1" }));
    expect(again).toBe(first);
  });

  it("failed_carries_error_and_retains_it_against_late_progress", () => {
    // failed 带 error;失败后迟到的下载进度帧整体丢弃(error 保留,
    // 不被旧进度覆盖)。
    let s = updateStateFromEvents(
      null,
      ev({ state: "downloading", latest: "1.0.1", progress: 0.5 }),
    );
    s = updateStateFromEvents(s, ev({ state: "failed", latest: "1.0.1", error: "下载失败:超时" }));
    expect(s!.state).toBe("failed");
    expect(s!.error).toBe("下载失败:超时");
    expect(s!.latest).toBe("1.0.1"); // daemon fail() 保留 latest(notes 同理)
    expect(s!.progress).toBeNull();

    const late = updateStateFromEvents(
      s,
      ev({ state: "downloading", latest: "1.0.1", progress: 0.9 }),
    );
    expect(late).toBe(s); // 迟到进度帧:原引用返回

    // 重试面:failed → downloading(翻转帧,progress=null)合法采纳,
    // error 清空;后续真实进度帧照常跟进。
    const retry = updateStateFromEvents(s, ev({ state: "downloading", latest: "1.0.1" }));
    expect(retry!.state).toBe("downloading");
    expect(retry!.error).toBeNull();
    const retryProgress = updateStateFromEvents(
      retry,
      ev({ state: "downloading", latest: "1.0.1", progress: 0.1 }),
    );
    expect(retryProgress!.progress).toBe(0.1);
  });

  it("stale_frames_after_ready_and_installing_are_discarded", () => {
    // ready 后迟到的下载帧(翻转帧或进度帧)整体丢弃:daemon 的 apply
    // 自 ready 幂等返回,不会再落 downloading。
    const ready = updateStateFromEvents(null, ev({ state: "ready", latest: "1.0.1" }));
    expect(
      updateStateFromEvents(ready, ev({ state: "downloading", latest: "1.0.1" })),
    ).toBe(ready);
    expect(
      updateStateFromEvents(ready, ev({ state: "downloading", latest: "1.0.1", progress: 0.9 })),
    ).toBe(ready);

    // installing 终态粘滞:daemon 不再离开 installing(安装序列收尾即
    // 退出),其后任何帧(重连重放/迟到翻转)一律忽略。
    const installing = updateStateFromEvents(ready, ev({ state: "installing", latest: "1.0.1" }));
    expect(updateStateFromEvents(installing, ev({ state: "available", latest: "1.0.2" }))).toBe(
      installing,
    );
    expect(updateStateFromEvents(installing, ev({ state: "idle" }))).toBe(installing);
    expect(
      updateStateFromEvents(installing, ev({ state: "failed", error: "迟到失败" })),
    ).toBe(installing);
    expect(
      updateStateFromEvents(installing, ev({ state: "installing", latest: "1.0.2" })),
    ).toBe(installing);
  });

  it("regressions_via_new_check_are_applied", () => {
    // 合法回退:available 下重查无新版 → idle(NOT_FOUND/版本相同);
    // failed 后重查得新版 → available。检查入口对任意态都权威。
    const available = updateStateFromEvents(null, ev({ state: "available", latest: "1.0.1" }));
    const idle = updateStateFromEvents(available, ev({ state: "idle" }));
    expect(idle!.state).toBe("idle");
    expect(idle!.latest).toBeNull();

    const failed = updateStateFromEvents(idle, ev({ state: "failed", error: "清单验签失败" }));
    const recovered = updateStateFromEvents(failed, ev({ state: "available", latest: "1.0.1" }));
    expect(recovered!.state).toBe("available");
    expect(recovered!.error).toBeNull();
  });

  it("adopts_first_frame_and_normalizes_optional_fields", () => {
    // 首帧整体采纳(null prev);省略字段归一为 null(非 undefined),
    // progress 越界夹回 [0,1]。
    const s = updateStateFromEvents(null, {
      state: "downloading",
      current: "1.0.0",
      latest: "1.0.1",
      progress: 1.5,
    });
    expect(s).toEqual({
      state: "downloading",
      current: "1.0.0",
      latest: "1.0.1",
      notes: null,
      progress: 1,
      error: null,
    });
    const neg = updateStateFromEvents(null, { state: "downloading", current: "1.0.0", progress: -1 });
    expect(neg.progress).toBe(0);
  });
});

describe("canInstall(安装按钮门控)", () => {
  it("installable_states_matrix", () => {
    // 可点三态:available(确认安装,D4 中档手动确认面)/ ready(下载
    // 就绪)/ pending_install(空闲门放行后立即安装)。其余态一律不可点:
    // downloading/applying/installing 进行中,idle 无可装,failed 走重试。
    expect(canInstall("available")).toBe(true);
    expect(canInstall("ready")).toBe(true);
    expect(canInstall("pending_install")).toBe(true);
    expect(canInstall("idle")).toBe(false);
    expect(canInstall("downloading")).toBe(false);
    expect(canInstall("applying")).toBe(false);
    expect(canInstall("installing")).toBe(false);
    expect(canInstall("failed")).toBe(false);
    // 快照形态(取 state 字段)同判定。
    const snap: UpdateSnapshot = {
      state: "pending_install",
      current: "1.0.0",
      latest: "1.0.1",
      notes: null,
      progress: null,
      error: null,
    };
    expect(canInstall(snap)).toBe(true);
  });
});

describe("updateTierLabel(状态文案映射)", () => {
  it("tier_label_mapping", () => {
    // 全态映射 i18n key(文案留在 zh.json,纯函数不做翻译);
    // applying(spec D11 事件名)与 installing 同文案。
    expect(updateTierLabel("idle")).toBe("update.tier.idle");
    expect(updateTierLabel("available")).toBe("update.tier.available");
    expect(updateTierLabel("downloading")).toBe("update.tier.downloading");
    expect(updateTierLabel("ready")).toBe("update.tier.ready");
    expect(updateTierLabel("pending_install")).toBe("update.tier.pendingInstall");
    expect(updateTierLabel("applying")).toBe("update.tier.installing");
    expect(updateTierLabel("installing")).toBe("update.tier.installing");
    expect(updateTierLabel("failed")).toBe("update.tier.failed");
    expect(updateTierLabel({ state: "ready" } as UpdateSnapshot)).toBe("update.tier.ready");
  });
});

describe("formatProgress(进度格式化)", () => {
  it("progress_formatting", () => {
    expect(formatProgress(null)).toBe("…"); // 翻转帧无进度:不确定态文案
    expect(formatProgress(0)).toBe("0%");
    expect(formatProgress(0.5)).toBe("50%");
    expect(formatProgress(0.999)).toBe("100%");
    expect(formatProgress(1)).toBe("100%");
    expect(formatProgress(1.5)).toBe("100%"); // 防御:越界夹回
    expect(formatProgress(-0.2)).toBe("0%");
  });
});
