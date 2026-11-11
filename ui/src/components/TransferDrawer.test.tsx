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
      onPause: noop,
      onResume: noop,
      onCancel: noop,
      onReinitiate: noop,
      onClear: noop,
    }),
  );

describe("方向列词面(SSR 冒烟,终审修复波)", () => {
  it("swap 图标方向(remote_copy/local_copy)词面=「调度」,全行无原始键面", () => {
    for (const kind of ["remote_copy", "local_copy"]) {
      const html = renderRow(kind);
      // 方向单元格 title 恰为方向词(钉住单元格本身;类型徽章「调度」
      // 另有一处,不足以单独钉住方向列)。
      expect(html, kind).toContain('title="调度"');
      expect(html, kind).toContain("调度");
      expect(html, kind).not.toContain("drawer.dir.");
    }
  });

  it("recv/send 方向词面照常解析(发送/接收),分键未伤及既有路径", () => {
    expect(renderRow("upload")).toContain('title="发送"');
    expect(renderRow("download")).toContain('title="接收"');
    expect(renderRow("incoming")).toContain('title="接收"');
    expect(renderRow("upload")).not.toContain("drawer.dir.");
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
    // 新到卡(devB:9)<dev>:<seq> 形)插队最顶:渲染首行 = 最新任务,
    // 行序与 reducer 收敛后的 store 序一致(arrival = recency)。
    let jobs = seedJobs([job("upload"), job("incoming")]);
    jobs = applyTransferEvents(jobs, frameOf({ id: "devB:9", kind: "incoming" }));
    const html = renderToStaticMarkup(createElement(TransferDrawer, drawerProps(jobs)));
    expect(rowIds(html)).toEqual(["devB:9", "job-incoming", "job-upload"]);
  });
});
