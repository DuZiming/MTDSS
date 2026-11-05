/// 常用目录管理屏 SSR 冒烟测试(V3-3 Task 13,沿 FavMemberPicker.test.tsx
/// 同款手法):受控组件,react-dom/server 纯字符串渲染,无需 DOM;i18n 经
/// ../i18n 同步初始化(zh 词表内联,与 main.tsx 同装配)。钉死:open=false
/// 不渲染;open 态成员行 = 设备名 + 路径 + 逐行「移除」钮;设备不在快照 →
/// 「设备已移除」灰标注(且不漏原始设备 id);底部删除钮 danger 类 +
/// 「删除常用目录」词面;busy=true 全部按钮禁用(逐行移除 + 底部删除)。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import "../i18n";

import type { DeviceInfo } from "../lib/derived";
import type { Favorite } from "../lib/favorites";
import FavManageDialog from "./FavManageDialog";

const noop = () => {};

function dev(id: string): DeviceInfo {
  return { device_id: id, name: `${id}-名`, platform: "windows", online: true };
}

function fav(members: { device: string; path: string }[]): Favorite {
  return { id: "f1", name: "合同扫描件", created_at: 1000, members };
}

const render = (p: {
  open: boolean;
  busy?: boolean;
  members?: { device: string; path: string }[];
  devices?: DeviceInfo[];
}) =>
  renderToStaticMarkup(
    createElement(FavManageDialog, {
      open: p.open,
      fav: fav(p.members ?? [{ device: "d1", path: "C:\\相册" }]),
      devices: p.devices ?? [dev("d1"), dev("d2")],
      colors: new Map<string, string>([
        ["d1", "#14B8A6"],
        ["d2", "#06B6D4"],
      ]),
      busy: p.busy ?? false,
      onRemoveMember: noop,
      onDelete: noop,
      onClose: noop,
    }),
  );

/// 完整按钮元素集(词面在按钮内容里,整元素取出断言 disabled/danger 类)。
function buttons(html: string): string[] {
  return [...html.matchAll(/<button[^>]*>(?:(?!<\/button>)[\s\S])*?<\/button>/g)].map(
    (m) => m[0],
  );
}

/// 该词面所属按钮的开标签(词在按钮内容里,向前找最近的 <button)。
function buttonOpenTag(html: string, labelIdx: number): string {
  return html.slice(html.lastIndexOf("<button", labelIdx), labelIdx);
}

describe("FavManageDialog(SSR 冒烟:成员行/设备已移除/危险删除钮/busy 禁用)", () => {
  it("关闭态:不渲染遮罩与任何词面", () => {
    expect(render({ open: false })).toBe("");
  });

  it("open 态:标题 + 成员行(设备名 + 路径 + 移除钮)+ 底部删除钮,全 zh 词面", () => {
    const html = render({ open: true });
    expect(html).toContain("管理常用目录");
    expect(html).toContain("d1-名");
    expect(html).toContain("C:\\相册");
    expect(html).toContain("#14B8A6"); // 色点内联背景
    expect(html).toContain("移除");
    expect(html).toContain("删除常用目录");
    expect(html).not.toContain("favorites.");
  });

  it("设备不在快照:行名切「设备已移除」灰标注,不漏原始设备 id", () => {
    const html = render({
      open: true,
      members: [{ device: "d9", path: "/tmp" }],
      devices: [dev("d1")],
    });
    expect(html).toContain("设备已移除");
    expect(html).not.toContain("d9");
    expect(html).toContain("removed"); // 灰标注类(.fav-manage-name.removed)
  });

  it("底部删除钮 = danger 类 +「删除常用目录」词面", () => {
    const html = render({ open: true });
    const tag = buttonOpenTag(html, html.indexOf("删除常用目录"));
    expect(tag).toContain("danger");
  });

  it("busy=true:逐行移除钮与底部删除钮全部禁用", () => {
    const html = render({
      open: true,
      busy: true,
      members: [
        { device: "d1", path: "C:\\a" },
        { device: "d2", path: "/b" },
      ],
    });
    for (const label of ["移除", "删除常用目录"]) {
      const hits = buttons(html).filter((b) => b.includes(label));
      expect(hits.length).toBeGreaterThan(0);
      for (const b of hits) expect(b).toContain("disabled");
    }
  });
});
