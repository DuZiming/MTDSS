/// 传输抽屉方向列 SSR 冒烟测试(终审修复波):swap 图标方向(remote_copy/
/// local_copy/未知种类)词面必须解析为「调度」(drawer.dir.ctl),不得漏出
/// 原始键面 "drawer.dir.*"——修复前 dirOf 的图标键被直拼进词面键,而 zh 词
/// 表只有 recv/send/ctl 三词(spec §12),每行方向列渲染成字面
/// "drawer.dir.swap"。react-dom/server 纯字符串渲染,无需 DOM;i18n 经
/// ../i18n 同步初始化(zh 词表内联,与 main.tsx 同一装配)。
/// v3p6 Task 16(spec §5.1-4)增列表序面:展开表行序 = store 迭代序
/// (reducer 保序,最新在顶),混合命名空间 id 集(裸数字 + <dev>:<seq>)
/// 渲染序与 store 序逐行一致;新到卡插队最顶。localStorage 桩让开合记忆
/// 读到 open:true(SSR 无存储,缺省收起不渲染表体)。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import "../i18n";

import {
  applyTransferEvents,
  seedJobs,
  type JobSnapUI,
  type TransferFrame,
} from "../lib/transfer";
import TransferDrawer, { DrawerRow } from "./TransferDrawer";

/// 最小作业快照:方向列只消费 kind;状态取 running 让行面走常态路径,
/// 其余字段按 JobSnapUI 契约补齐。
const job = (kind: string): JobSnapUI => ({
  id: `job-${kind}`,
  kind,
  state: "running",
  files_done: 0,
  files_total: 1,
  bytes_done: 0,
  bytes_total: 1,
  speed_bps: 0,
  relay: false,
  error: null,
  src: "D:\\a",
  dst: "D:\\b",
});

const noop = () => {};

/// 渲染单行 SSR 输出(操作回调全 noop,断言面只看方向列)。
const renderRow = (kind: string) =>
  renderToStaticMarkup(
    createElement(DrawerRow, {
      job: job(kind),
      busy: false,
      nameOf: (id: string) => ({ "dev-a": "工位机" }[id]),
      onPause: noop,
      onResume: noop,
      onCancel: noop,
      onReinitiate: noop,
      onClear: noop,
    }),
  );

describe("方向列词面(SSR 冒烟,终审修复波+2026-11-16 设备对)", () => {
  it("解析不出设备对(路径形/缺种子)回退方向词,全行无原始键面", () => {
    // 夹具 src/dst = "D:" 本地路径形:hex 校验拒识 → 回退图标+词
    // (local_copy 不在此列:双端本机恒有真对「本机 → 本机」)
    for (const kind of ["remote_copy"]) {
      const html = renderRow(kind);
      expect(html, kind).toContain('title="调度"');
      expect(html, kind).toContain("调度");
      expect(html, kind).not.toContain("drawer.dir.");
    }
    expect(renderRow("local_copy")).toContain("本机 → 本机");
    expect(renderRow("upload")).toContain('title="发送"');
    expect(renderRow("download")).toContain('title="接收"');
    expect(renderRow("incoming")).toContain('title="接收"');
    expect(renderRow("upload")).not.toContain("drawer.dir.");
  });

  it("设备对解析(2026-11-16):种子形 src/dst → 图标+「名 → 名」", () => {
    const DEVA = "a".repeat(32);
    const DEVB = "b".repeat(32);
    const rowOf = (over: Partial<JobSnapUI>): string =>
      renderToStaticMarkup(
        createElement(DrawerRow, {
          job: { ...job("upload"), ...over },
          busy: false,
          nameOf: (id: string) =>
            ({ [DEVA]: "工位机", [DEVB]: "仓库机" })[id],
          onPause: noop,
          onResume: noop,
          onCancel: noop,
          onReinitiate: noop,
          onClear: noop,
        }),
      );
    // upload:本机 → 目标设备(dst="dev:dir")
    expect(rowOf({ dst: `${DEVB}:C:\in` })).toContain("本机 → 仓库机");
    // download:源设备 → 本机(src="dev:path,…")
    expect(rowOf({ kind: "download", src: `${DEVA}:/srv/x, /srv/y` })).toContain(
      "工位机 → 本机",
    );
    // incoming:裸 id 来源 → 本机
    expect(rowOf({ kind: "incoming", src: DEVA })).toContain("工位机 → 本机");
    // remote_copy:双端设备
    expect(
      rowOf({ kind: "remote_copy", src: `${DEVA}:/x`, dst: `${DEVB}:/y` }),
    ).toContain("工位机 → 仓库机");
    // 未知设备(快照缺):回退裸 id 不硬造
    expect(rowOf({ dst: `${"c".repeat(32)}:C:\in` })).toContain(
      `${"c".repeat(32)}`,
    );
  });
});

// ---------- r51 #2a:收起条端点解析(方向列同款) ----------

describe("收起条设备名解析(r51 #2a,SSR)", () => {
  beforeEach(() => {
    lsStore.clear(); // 缺省收起(SSR 不跑 effects,恒渲染收起条)
  });

  it("collapsed_pair_resolves_device_names_not_raw_ids", () => {
    // upload:dst="dev:dir" 种子形 → 收起条显「本机 → 仓库机」,
    // 正文不晒 32 位 hex 裸 id;title 恒携裸形(悬停可查原始 src/dst)
    const DEVB = "b".repeat(32);
    const jobs = seedJobs([{ ...job("upload"), dst: `${DEVB}:C:\\in` }]);
    const html = renderToStaticMarkup(
      createElement(TransferDrawer, {
        ...drawerProps(jobs),
        nameOf: (id: string) => ({ [DEVB]: "仓库机" })[id],
      }),
    );
    expect(html).toContain("本机 → 仓库机");
    expect(html).toContain(`title="D:\\a → ${DEVB}:C:\\in"`);
  });

  it("collapsed_live_row_without_endpoints_falls_back_to_legacy", () => {
    // r51 #2b 同源面:端点解析不出(路径形/旧 daemon live 帧无 src/dst)
    // → 回退旧裸形「src → dst」(不硬造)
    const jobs = seedJobs([job("upload")]); // 夹具 src/dst = 本地路径形
    const html = renderToStaticMarkup(createElement(TransferDrawer, drawerProps(jobs)));
    expect(html).toContain("D:\\a");
    expect(html).toContain("D:\\b");
  });
});

// ---------- v3p6 Task 16(spec §5.1-4):列表最新在上 ----------

/// 最小事件帧(event.transfer 形态:不带 src/dst,抽屉新到卡路径)。
const frameOf = (over: Partial<TransferFrame> & { id: string }): TransferFrame => ({
  kind: "incoming",
  state: "running",
  files_done: 0,
  files_total: 1,
  bytes_done: 0,
  bytes_total: 1,
  speed_bps: 0,
  error: null,
  ...over,
});

/// localStorage 桩(node 环境无存储):开合记忆预置 open:true,SSR 才渲染
/// 展开表体;写入收进内存 Map,测试间互不渗漏。
const lsStore = new Map<string, string>();
const lsStub = {
  getItem: (k: string) => lsStore.get(k) ?? null,
  setItem: (k: string, v: string) => void lsStore.set(k, v),
  removeItem: (k: string) => void lsStore.delete(k),
  clear: () => void lsStore.clear(),
  key: (i: number) => [...lsStore.keys()][i] ?? null,
  get length() {
    return lsStore.size;
  },
};

/// 全量 drawer props(回调 noop;SSR 不跑 effects,回调不触达)。
const drawerProps = (jobs: ReadonlyMap<string, JobSnapUI>) => ({
  jobs,
  busyIds: new Set<string>(),
  nameOf: (_id: string) => undefined,
  onPause: noop,
  onResume: noop,
  onCancel: noop,
  onReinitiate: noop,
  onClear: noop,
});

/// 展开 SSR 输出里 data-job-id 的出现序 = 抽屉行面序。
const rowIds = (html: string): string[] =>
  [...html.matchAll(/data-job-id="([^"]*)"/g)].map((m) => m[1]);

describe("展开表行序 = store 序,最新在顶(v3p6 Task 16)", () => {
  beforeEach(() => {
    lsStore.clear();
    lsStore.set("mtdss.drawer", JSON.stringify({ open: true, height: 0.25 }));
    vi.stubGlobal("localStorage", lsStub);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("rendered_row_order_matches_store_order_for_mixed_ids", () => {
    // 混合命名空间集(daemon 契约升序种子:裸数字 upload/download 与
    // <dev>:<seq> incoming)→ 渲染行序与 store 迭代序逐行一致(最新在上)
    const jobs = seedJobs([
      job("upload"),
      job("incoming"),
      job("download"),
    ]);
    const html = renderToStaticMarkup(createElement(TransferDrawer, drawerProps(jobs)));
    expect(rowIds(html)).toEqual([...jobs.keys()]);
    expect(rowIds(html)).toEqual(["job-download", "job-incoming", "job-upload"]);
  });

  it("new_arrival_row_lands_above_seeded_rows", () => {
    // 新到卡(devB:9,<dev>:<seq> 形)插队最顶:渲染首行 = 最新任务,
    // 行序与 reducer 收敛后的 store 序一致(arrival = recency)。
    let jobs = seedJobs([job("upload"), job("incoming")]);
    jobs = applyTransferEvents(jobs, frameOf({ id: "devB:9", kind: "incoming" }));
    const html = renderToStaticMarkup(createElement(TransferDrawer, drawerProps(jobs)));
    expect(rowIds(html)).toEqual(["devB:9", "job-incoming", "job-upload"]);
  });
});

// ---------- 2026-11-18 smoke 票⑥:终态进度条持久化 ----------
// 失败/中断/取消三态:160px 槽不再被错误文字占用,改显**中性冻结条**
// (fill 宽 = 最终 ratio,徽章承担状态色);百分比列显冻结百分比(不再
// 「—」);完整错误串退到槽区 title 悬停(仅 error 非空时);ratio 未知
// (扫描期失败/0 字节)空条 +「—」。用户裁定形态=中性冻结条(2026-11-18)。

describe("终态进度条持久化(smoke 票⑥,SSR)", () => {
  const rowOf = (over: Partial<JobSnapUI>): string =>
    renderToStaticMarkup(
      createElement(DrawerRow, {
        job: { ...job("upload"), ...over },
        busy: false,
        nameOf: () => undefined,
        onPause: noop,
        onResume: noop,
        onCancel: noop,
        onReinitiate: noop,
        onClear: noop,
      }),
    );

  it("failed_freezes_bar_at_final_ratio_with_error_hover", () => {
    // 37/100 失败:冻结条 37% + 百分比 37% + 槽区 title=完整错误串;
    // 错误文字不再占槽(st-reason 退场)
    const html = rowOf({
      state: "failed",
      bytes_done: 37,
      bytes_total: 100,
      error: "连接中断: 对端离线",
    });
    expect(html).toContain('role="progressbar"');
    expect(html).toContain("width:37%");
    expect(html).toContain('class="st-pct">37%<');
    expect(html).toContain('title="连接中断: 对端离线"');
    expect(html).not.toContain("st-reason");
  });

  it("interrupted_without_error_frozen_bar_no_hover", () => {
    // 无错中断:冻结条照显,title 不设(无信息可悬)
    const html = rowOf({ state: "interrupted", bytes_done: 31, bytes_total: 100 });
    expect(html).toContain("width:31%");
    expect(html).toContain('class="st-pct">31%<');
    expect(html).not.toContain("st-reason");
    expect(html).not.toMatch(/class="st-slot"[^>]*title=/);
  });

  it("cancelled_unknown_ratio_empty_bar_dash_pct", () => {
    // ratio 未知(bytes_total 0):空条(无 fill 宽)+「—」
    const html = rowOf({ state: "cancelled", bytes_done: 0, bytes_total: 0 });
    expect(html).toContain('role="progressbar"');
    expect(html).not.toContain("width:");
    expect(html).toContain('class="st-pct">—<');
  });

  it("cancelled_partial_ratio_freezes_bar", () => {
    // 取消于 8%:冻结条 8% + 百分比 8%
    const html = rowOf({ state: "cancelled", bytes_done: 8, bytes_total: 100 });
    expect(html).toContain("width:8%");
    expect(html).toContain('class="st-pct">8%<');
  });

  it("resume_button_renders_svg_triangle_not_text_glyph", () => {
    // smoke 票④:暂停行的「继续」钮改 SVG 三角(与其它操作钮同款几何
    // 居中);旧文字字形 ▶ 挂文本基线,视觉下移——不得再出现 act-glyph
    const html = rowOf({ state: "paused", bytes_done: 10, bytes_total: 100 });
    expect(html).toContain('aria-label="继续"');
    expect(html).not.toContain("act-glyph");
    expect(html).not.toContain("▶");
    // 继续钮内是 svg(与暂停/取消钮同族)
    const btn = html.slice(html.indexOf('aria-label="继续"'));
    expect(btn.slice(0, 200)).toContain("<svg");
  });
});
