/// 新建/重命名快捷目录对话框 SSR 冒烟测试(V3-3 Task 8,沿
/// ApprovalModal.test.tsx 同款手法):受控组件,react-dom/server 纯字符串
/// 渲染,无需 DOM;i18n 经 ../i18n 同步初始化(zh 词表内联,与 main.tsx
/// 同装配)。钉死:open=false 不渲染遮罩;open 态标题/输入初值/取消·提交
/// 两键全为 zh 词面(不漏原始键面);initial 有无切换 新建/重命名 两副
/// 词面;busy=true 提交钮禁用。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import "../i18n";

import FavNameDialog from "./FavNameDialog";

const noop = () => {};

const render = (p: { open: boolean; initial?: string; busy?: boolean }) =>
  renderToStaticMarkup(
    createElement(FavNameDialog, {
      open: p.open,
      initial: p.initial,
      busy: p.busy ?? false,
      onSubmit: noop,
      onCancel: noop,
    }),
  );

describe("FavNameDialog(SSR 冒烟:词面解析与开关面)", () => {
  it("关闭态:不渲染遮罩与任何词面", () => {
    expect(render({ open: false })).toBe("");
  });

  it("新建态(initial 缺省):标题/提交/取消全词面,无原始键面", () => {
    const html = render({ open: true });
    expect(html).toContain("新建快捷目录");
    expect(html).toContain("确定");
    expect(html).toContain("取消");
    expect(html).not.toContain("favorites.");
    expect(html).not.toContain("sidebar.");
  });

  it("重命名态(initial 非空):标题切「重命名」、提交钮切「保存」,输入预填初值", () => {
    const html = render({ open: true, initial: "合同扫描件" });
    expect(html).toContain("重命名");
    expect(html).not.toContain("新建快捷目录");
    expect(html).toContain("保存");
    expect(html).toContain('value="合同扫描件"');
  });

  it("busy=true:提交钮禁用(busy 在途防连点)", () => {
    const html = render({ open: true, busy: true });
    const submit = html.match(/<button[^>]*>确定<\/button>/)?.[0] ?? "";
    expect(submit).toContain("disabled");
  });
});
