/// 审批弹窗 SSR 冒烟测试(V3-1 Task 10,沿 TransferDrawer.test.tsx 同款
/// 手法):受控组件(req 传入),SSR 渲染钉死文案词面——标题/候选句/
/// 平台行/审批码大字/同意·拒绝两键全部解析为 zh 词表词,不漏原始键面
/// "approve.*";req=null 不渲染遮罩。react-dom/server 纯字符串渲染,
/// 无需 DOM;i18n 经 ../i18n 同步初始化(zh 词表内联,与 main.tsx 同装配)。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import "../i18n";

import type { ApprovalEvent } from "../lib/daemon";
import ApprovalModal from "./ApprovalModal";

const req: ApprovalEvent = {
  session_id: "sess-a",
  candidate_name: "契约申请端",
  candidate_platform: "windows",
  code: "123456",
  expires_at: 1760000200,
};

const noop = () => {};

const render = (r: ApprovalEvent | null) =>
  renderToStaticMarkup(
    createElement(ApprovalModal, { req: r, busy: false, onRespond: noop, onDismiss: noop }),
  );

describe("ApprovalModal(SSR 冒烟:词面解析与开关面)", () => {
  it("打开态:候选名/平台/6 位码大字/同意·拒绝全词面,无原始键面", () => {
    const html = render(req);
    expect(html).toContain("新终端登录请求");
    expect(html).toContain("「契约申请端」请求登录你的账号");
    expect(html).toContain("平台:windows");
    expect(html).toContain("审批码");
    expect(html).toContain("123456");
    expect(html).toContain("同意");
    expect(html).toContain("拒绝");
    expect(html).not.toContain("approve.");
  });

  it("关闭态(req=null):不渲染遮罩与任何词面", () => {
    expect(render(null)).toBe("");
  });
});
