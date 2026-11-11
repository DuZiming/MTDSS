import { describe, expect, it } from "vitest";

import {
  aggRate,
  applyTransferEvents,
  collapsedCounts,
  filterByKind,
  fmtSpeed,
  isTerminalState,
  jobDisplayName,
  pickCollapsedJob,
  progressRatio,
  seedJobs,
  type JobSnapUI,
  type TransferFrame,
} from "./transfer";

/// 合成一条 UI 快照(缺省字段填常规值,测试只关注目标位)。
function snap(over: Partial<JobSnapUI> & { id: string }): JobSnapUI {
  return {
    kind: "upload",
    state: "running",
    files_done: 0,
    files_total: 2,
    bytes_done: 0,
    bytes_total: 100,
    speed_bps: 0,
    relay: false,
    src: "C:\\a",
    dst: "B:\\b",
    error: null,
    ...over,
  };
}

/// 合成一条线上事件帧(event.transfer 形态:不带 src/dst)。
function frame(over: Partial<TransferFrame> & { id: string }): TransferFrame {
  const { src: _s, dst: _d, ...rest } = snap(over);
  return rest;
}

describe("applyTransferEvents(事件收敛 reducer)", () => {
  it("apply_events_dedupes_and_orders", () => {
    const prev = new Map<string, JobSnapUI>();
    let m = applyTransferEvents(prev, snap({ id: "2", bytes_done: 10 }));
    m = applyTransferEvents(m, snap({ id: "1", bytes_done: 5 }));
    // 同 id 重复事件:按 id 去重合并为一条,后者胜(最新快照)
    m = applyTransferEvents(m, snap({ id: "2", bytes_done: 99 }));
    // 后到者插队最前(最新在顶,v3p6 Task 16);既有键更新不翻转行序
    expect([...m.keys()]).toEqual(["1", "2"]);
    expect(m.get("2")!.bytes_done).toBe(99);
    expect(m.size).toBe(2);
    // 线上事件帧缺 src/dst:保留前值(卡片仍能显示来源去向)
    const m2 = applyTransferEvents(m, frame({ id: "2", bytes_done: 50 }));
    expect(m2.get("2")!.bytes_done).toBe(50);
    expect([...m2.keys()]).toEqual(["1", "2"]); // 更新仍不翻转
    expect(m2.get("2")!.src).toBe("C:\\a");
    expect(m2.get("2")!.dst).toBe("B:\\b");
    // 纯函数:入参 prev 不被改写
    expect(prev.size).toBe(0);
  });

  it("terminal_state_wins_over_late_progress", () => {
    let m = applyTransferEvents(new Map(), snap({ id: "1" }));
    m = applyTransferEvents(m, snap({ id: "1", state: "done", bytes_done: 100 }));
    expect(m.get("1")!.state).toBe("done");
    // 终态后的迟到进度/翻帧:整体忽略(终态粘滞,条目原样保留)
    const late = applyTransferEvents(
      m,
      snap({ id: "1", state: "running", bytes_done: 1, speed_bps: 77 }),
    );
    expect(late.get("1")).toBe(m.get("1"));
    // 终态之间的迟到翻转同样拒:done 之后迟到的 failed 不翻(先到者胜)
    const lateFail = applyTransferEvents(
      m,
      snap({ id: "1", state: "failed", error: "迟到错误" }),
    );
    expect(lateFail.get("1")!.state).toBe("done");
    expect(lateFail.get("1")!.error).toBeNull();
  });

  it("seed_jobs_from_transfer_list", () => {
    // transfer.list 全量种子(daemon 契约:id 升序 = 老在前):经同一
    // reducer 收敛后反转为**最新在上**(v3p6 Task 16,spec §5.1-4);
    // 终态条目照收(reducer 统一路径),由 App 侧把终态 id 挪进隐藏集,
    // 种子帧不弹历史卡。
    const m = seedJobs([
      snap({ id: "3", state: "running" }),
      snap({ id: "9", state: "done", bytes_done: 100 }),
    ]);
    expect([...m.keys()]).toEqual(["9", "3"]);
    expect(m.get("3")!.state).toBe("running");
  });
});

// ---------- v3p6 Task 16:抽屉最新在上(spec §5.1-4) ----------

describe("列表最新在上(v3p6 Task 16,spec §5.1-4)", () => {
  it("new_arrival_lands_on_top", () => {
    // 新到作业卡插队最顶(arrival = recency):种子后新增事件帧的 id
    // 排在既有行之前,抽屉首行即最新任务。
    const m = seedJobs([snap({ id: "1" }), snap({ id: "2" })]);
    expect([...m.keys()]).toEqual(["2", "1"]);
    const m2 = applyTransferEvents(m, frame({ id: "3" }));
    expect([...m2.keys()]).toEqual(["3", "2", "1"]);
  });

  it("update_in_place_does_not_reorder", () => {
    // 既有键更新(进度帧)原位改写:行序不翻转(抽屉行面稳定,无抖动)
    const m = seedJobs([snap({ id: "1" }), snap({ id: "2" })]);
    const m2 = applyTransferEvents(m, frame({ id: "1", bytes_done: 50 }));
    expect([...m2.keys()]).toEqual(["2", "1"]);
    expect(m2.get("1")!.bytes_done).toBe(50);
  });

  it("mixed_namespace_order_stays_arrival_based", () => {
    // 跨命名空间(裸数字 id 与 <dev>:<seq> 混存)不按数字裸序错排:
    // 种子按 daemon 升序契约输入 → 反转展示;新到 incoming 卡
    // (<dev>:<seq> 形)arrival 即最新,插队最顶。
    const m = seedJobs([
      snap({ id: "1", kind: "upload" }),
      snap({ id: "devA:2", kind: "incoming" }),
      snap({ id: "5", kind: "download" }),
    ]);
    expect([...m.keys()]).toEqual(["5", "devA:2", "1"]);
    const m2 = applyTransferEvents(m, frame({ id: "devB:9", kind: "incoming" }));
    expect([...m2.keys()]).toEqual(["devB:9", "5", "devA:2", "1"]);
  });
});

describe("relay(中继兜底标记)", () => {
  it("relay_true_passes_through_snapshot", () => {
    // 新 daemon 事件帧带 relay:true → 快照透传(卡片角标数据源)
    const m = applyTransferEvents(new Map(), frame({ id: "1", relay: true }));
    expect(m.get("1")!.relay).toBe(true);
  });

  it("relay_missing_field_falls_back_false", () => {
    // 旧 daemon 事件帧无 relay 字段(v2-P1c 之前形态)→ 容错为 false,
    // 不崩、不误显角标。
    const { relay: _dropped, ...oldFrame } = frame({ id: "2" });
    expect("relay" in oldFrame).toBe(false);
    const m = applyTransferEvents(new Map(), oldFrame);
    expect(m.get("2")!.relay).toBe(false);
  });
});

describe("fmtSpeed(速率格式化)", () => {
  it("fmt_speed_units", () => {
    expect(fmtSpeed(0)).toBe("0 B/s");
    expect(fmtSpeed(1023)).toBe("1023 B/s");
    expect(fmtSpeed(1024)).toBe("1.0 KB/s");
    expect(fmtSpeed(1536)).toBe("1.5 KB/s");
    expect(fmtSpeed(1024 * 1024)).toBe("1.0 MB/s");
    expect(fmtSpeed(Math.round(1.2 * 1024 * 1024))).toBe("1.2 MB/s");
    expect(fmtSpeed(1024 ** 3)).toBe("1.0 GB/s");
    expect(fmtSpeed(3 * 1024 ** 3)).toBe("3.0 GB/s");
  });
});

describe("aggRate(状态栏汇总)", () => {
  it("agg_rate_sums_active", () => {
    const jobs = [
      snap({ id: "1", state: "running", speed_bps: 100 }),
      snap({ id: "2", state: "running", speed_bps: 250 }),
      snap({ id: "3", state: "paused", speed_bps: 999 }), // 暂停陈旧速度不计入速率
      snap({ id: "4", state: "done", speed_bps: 500 }), // 终态不计速也不计数
    ];
    expect(aggRate(jobs)).toEqual({ down_bps: 350, active: 3 });
    expect(aggRate([])).toEqual({ down_bps: 0, active: 0 });
  });
});

describe("卡片辅助纯函数", () => {
  it("progress_ratio_and_terminal_states", () => {
    // 总量已知 → 比值;总量 0(远端计数未知,download/remote_copy/incoming)
    // → null(卡片走不确定态)
    expect(progressRatio(50, 100)).toBe(0.5);
    expect(progressRatio(100, 100)).toBe(1);
    expect(progressRatio(0, 0)).toBeNull();
    // 终态判定(与 daemon is_terminal_state 同口径)
    expect(isTerminalState("done")).toBe(true);
    expect(isTerminalState("failed")).toBe(true);
    expect(isTerminalState("cancelled")).toBe(true);
    expect(isTerminalState("running")).toBe(false);
    expect(isTerminalState("paused")).toBe(false);
    expect(isTerminalState("queued")).toBe(false);
  });
});

// ---------- v2-P2 Task 9:传输抽屉(持久历史面) ----------

describe("interrupted 终态判定(v2-P2 Task 9)", () => {
  it("interrupted_is_terminal_state", () => {
    // 重启中断标记(spec §3):抽屉侧视为终态——无 live 驱动、无事件可来,
    // 行面按历史留置(重新发起/清除),速率汇总不计入「传输中」。
    expect(isTerminalState("interrupted")).toBe(true);
  });

  it("interrupted_seed_sticks_over_late_events", () => {
    // interrupted 种子行粘滞:迟到的进度/翻帧整体忽略(终态语义同 done/failed)
    const m = applyTransferEvents(
      new Map(),
      snap({ id: "1", state: "interrupted" }),
    );
    const late = applyTransferEvents(
      m,
      snap({ id: "1", state: "running", bytes_done: 1, speed_bps: 77 }),
    );
    expect(late.get("1")).toBe(m.get("1"));
  });

  it("agg_rate_excludes_interrupted", () => {
    // interrupted 已停摆:不计数、不计速(收起摘要条「进行中 N」不含它)
    expect(
      aggRate([snap({ id: "1", state: "interrupted", speed_bps: 50 })]),
    ).toEqual({ down_bps: 0, active: 0 });
  });
});

describe("filterByKind(抽屉筛选四档,v2-P2 Task 9)", () => {
  it("filter_by_kind_four_ways", () => {
    const jobs = [
      snap({ id: "1", kind: "upload" }),
      snap({ id: "2", kind: "download" }),
      snap({ id: "3", kind: "incoming" }),
      snap({ id: "4", kind: "remote_copy" }),
    ];
    expect(filterByKind(jobs, "all").map((j) => j.id)).toEqual(["1", "2", "3", "4"]);
    expect(filterByKind(jobs, "upload").map((j) => j.id)).toEqual(["1"]);
    // 接收档:download 与被动 incoming 同为「接收」行(spec §2/§7.2)
    expect(filterByKind(jobs, "download").map((j) => j.id)).toEqual(["2", "3"]);
    expect(filterByKind(jobs, "remote_copy").map((j) => j.id)).toEqual(["4"]);
  });

  it("filter_by_kind_keeps_order_and_does_not_mutate", () => {
    const jobs = [
      snap({ id: "2", kind: "download" }),
      snap({ id: "1", kind: "upload" }),
    ];
    const out = filterByKind(jobs, "upload");
    expect(out.map((j) => j.id)).toEqual(["1"]); // 首见序保持
    expect(jobs.map((j) => j.id)).toEqual(["2", "1"]); // 纯函数:入参不动
  });
});

describe("jobDisplayName(任务名直读收敛,V3-1 Task 10)", () => {
  it("name 非空直读(daemon 代表名含「等 N 项」后缀,UI 不再自行拼装)", () => {
    expect(jobDisplayName({ id: "9", name: "a.txt" })).toBe("a.txt");
    // 多项形态:Task 9 起代表名由 daemon 单源计算,UI 原样透显
    expect(jobDisplayName({ id: "9", name: "报告.pdf 等 3 项" })).toBe("报告.pdf 等 3 项");
  });

  it("name 缺省/空串回退 id(旧 daemon 帧无 name;病态输入 daemon 自算空名)", () => {
    expect(jobDisplayName({ id: "9" })).toBe("9");
    expect(jobDisplayName({ id: "9", name: "" })).toBe("9");
  });

  it("name_survives_event_frames_without_name", () => {
    // name 在事件帧缺席时(旧 daemon 进度帧)reducer 保留前值,抽屉任务名
    // 列不因进度事件翻成 id。
    const m = applyTransferEvents(new Map(), snap({ id: "1", name: "a.txt" }));
    const m2 = applyTransferEvents(m, frame({ id: "1", bytes_done: 1 }));
    expect(m2.get("1")!.name).toBe("a.txt");
    // 全新 id 且无 name:缺省 undefined(展示层回退 id)
    const m3 = applyTransferEvents(m2, frame({ id: "2" }));
    expect(m3.get("2")!.name).toBeUndefined();
  });
});

// ---------- P4b Task 8:收起条选任务与计数(spec §10 用户裁定) ----------

describe("pickCollapsedJob(收起条选任务,P4b Task 8)", () => {
  it("active_jobs_pick_newest_in_store_order", () => {
    // 多活动取最新建卡者(v3p6 Task 16 起迭代序 = 最新在顶,首个活动胜)
    const m = seedJobs([
      snap({ id: "1", state: "running" }),
      snap({ id: "2", state: "paused" }),
      snap({ id: "3", state: "queued" }),
    ]);
    expect(pickCollapsedJob(m)!.id).toBe("3");
  });

  it("active_beats_terminal_even_when_terminal_is_newer", () => {
    // 活动+终态混合取活动(更新的终态行也不夺位)
    const m = seedJobs([
      snap({ id: "1", state: "done", bytes_done: 100 }),
      snap({ id: "2", state: "running" }),
      snap({ id: "3", state: "failed" }),
    ]);
    expect(pickCollapsedJob(m)!.id).toBe("2");
  });

  it("all_terminal_picks_newest_terminal", () => {
    // 全终态取最新历史行(迭代序首者;interrupted 同终态门)
    const m = seedJobs([
      snap({ id: "1", state: "done" }),
      snap({ id: "2", state: "interrupted" }),
      snap({ id: "3", state: "cancelled" }),
    ]);
    expect(pickCollapsedJob(m)!.id).toBe("3");
  });

  it("empty_map_returns_null", () => {
    // 空 Map → null(收起条显示「传输 · 暂无传输任务」)
    expect(pickCollapsedJob(new Map())).toBeNull();
  });

  it("update_does_not_flip_order_and_input_untouched", () => {
    // 顺序假设的文档面(见 pickCollapsedJob 注):reducer 对既有键 set
    // 不翻转迭代序(最新在顶),更新不改位;纯函数:入参 Map 不被改写。
    const m0 = applyTransferEvents(new Map(), snap({ id: "a", state: "running" }));
    const m1 = applyTransferEvents(m0, snap({ id: "b", state: "queued" }));
    const m2 = applyTransferEvents(m1, snap({ id: "a", state: "running", bytes_done: 50 }));
    expect([...m2.keys()]).toEqual(["b", "a"]);
    expect(pickCollapsedJob(m2)!.id).toBe("b");
    expect(m1.size).toBe(2);
    expect(m0.size).toBe(1);
  });
});

describe("collapsedCounts(收起条计数,P4b Task 8)", () => {
  it("counts_three_segments_over_mixed_states", () => {
    // 三段口径:active=非终态(queued/running/paused,aggRate 同门);
    // interrupted/failed 各自成段;done/cancelled 不进任何段。
    const m = seedJobs([
      snap({ id: "1", state: "running" }),
      snap({ id: "2", state: "paused" }),
      snap({ id: "3", state: "queued" }),
      snap({ id: "4", state: "interrupted" }),
      snap({ id: "5", state: "failed" }),
      snap({ id: "6", state: "done" }),
      snap({ id: "7", state: "cancelled" }),
    ]);
    expect(collapsedCounts(m)).toEqual({ active: 3, interrupted: 1, failed: 1 });
  });

  it("empty_map_all_zero", () => {
    expect(collapsedCounts(new Map())).toEqual({ active: 0, interrupted: 0, failed: 0 });
  });

  it("terminal_only_zero_active", () => {
    // 全终态(含 done/cancelled 这类不进段的)→ active=0(零段/整徽章
    // 隐藏由组件侧处理,本函数只如实报数)
    const m = seedJobs([
      snap({ id: "1", state: "done" }),
      snap({ id: "2", state: "cancelled" }),
    ]);
    expect(collapsedCounts(m)).toEqual({ active: 0, interrupted: 0, failed: 0 });
  });

  it("interrupted_counts_as_interrupted_not_active", () => {
    // 「中断」口径(与旧摘要条同门):interrupted 是终态,不计活动,
    // 单独成段
    const m = seedJobs([snap({ id: "1", state: "interrupted" })]);
    expect(collapsedCounts(m)).toEqual({ active: 0, interrupted: 1, failed: 0 });
  });
});
