/// 传输抽屉方向列 SSR 冒烟测试(终审修复波):swap 图标方向(remote_copy/
/// local_copy/未知种类)词面必须解析为「调度」(drawer.dir.ctl),不得漏出
/// 原始键面 "drawer.dir.*"——修复前 dirOf 的图标键被直拼进词面键,而 zh 词
/// 表只有 recv/send/ctl 三词(spec §12),每行方向列渲染成字面
/// "drawer.dir.swap"。react-dom/server 纯字符串渲染,无需 DOM;i18n 经
/// ../i18n 同步初始化(zh 词表内联,与 main.tsx 同一装配)。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import "../i18n";

import type { JobSnapUI } from "../lib/transfer";
import { DrawerRow } from "./TransferDrawer";

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
