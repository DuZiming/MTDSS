/// 右键菜单「快速发送到」测试(spec §5.2,裁量 #11)。测试设施为 node 环境
/// vitest(无 jsdom/@testing-library,不为此引 jsdom):纯函数契约 +
/// react-dom/server SSR 冒烟(沿 FavMemberPicker.test.tsx 手法)。飞出
/// 开态经 QuickSendFlyout 直渲钉面(SSR 无 DOM 交互);点击「恰一次、
/// 无确认门」契约经 quickSendItems 装配函数直调钉死(ContextMenu 渲染
/// 消费同一装配,两臂零漂移)。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import "../i18n";

import type { DeviceInfo } from "../lib/derived";
import ContextMenu, {
  QuickSendFlyout,
  quickSendItems,
  quickSendTargets,
  type CtxMenuState,
} from "./ContextMenu";

const noop = () => {};

/// node 环境无 window:菜单/飞出定位读 innerWidth/innerHeight,SSR 冒烟
/// 挂最小桩(沿 localStorage 同款 Object.defineProperty 手法,不为此引
/// jsdom)。
Object.defineProperty(globalThis, "window", {
  value: { innerWidth: 1280, innerHeight: 800 },
  writable: true,
  configurable: true,
});

function dev(id: string, online: boolean, name?: string | null): DeviceInfo {
  return {
    device_id: id,
    name: name === undefined ? `${id}-名` : name,
    platform: "windows",
    online,
  };
}

/// 条目菜单形态(文件行;canOpen/canAddFav false = 文件/多选同形——入口
/// 不依赖选择数,多选门在 App 侧作用域)。
const entryMenu: CtxMenuState = {
  x: 10,
  y: 20,
  target: { name: "a.txt", path: "C:\\d\\a.txt", size: 3, mtime_ms: 1, is_dir: false },
};

const blankMenu: CtxMenuState = { x: 10, y: 20, target: null };

const baseProps = {
  canOpen: false,
  canPaste: true,
  canDownload: false,
  canAddFav: false,
  onClose: noop,
  onOpen: noop,
  onCut: noop,
  onCopy: noop,
  onPaste: noop,
  onDelete: noop,
  onDownloadToLocal: noop,
  onAddFav: noop,
  onRefresh: noop,
};

const OFFLINE_HINT = "设备离线,不可发送";

const renderMenu = (p: {
  menu?: CtxMenuState;
  quickSend?: { devices: DeviceInfo[]; selfDeviceId: string | null; onPick(id: string): void };
}) =>
  renderToStaticMarkup(
    createElement(ContextMenu, {
      ...baseProps,
      menu: p.menu ?? entryMenu,
      quickSend: p.quickSend,
    }),
  );

describe("quickSendTargets(目标装配:全部远端/本机剔除/离线置灰)", () => {
  it("全部远端设备列出,本机(selfDeviceId 命中)剔除;序沿 sortDevices(名称序)", () => {
    const out = quickSendTargets(
      [dev("self-1", true), dev("台式机", true), dev("笔记本", false)],
      "self-1",
    );
    expect(out.map((t) => t.device_id)).toEqual(["笔记本", "台式机"]);
  });

  it("selfDeviceId null(未登录防御)→ 全部列出", () => {
    const out = quickSendTargets([dev("d1", true), dev("d2", false)], null);
    expect(out.map((t) => t.device_id)).toEqual(["d1", "d2"]);
  });

  it("离线设备 disabled(灰显非隐藏),在线可用", () => {
    const out = quickSendTargets([dev("d1", true), dev("d2", false)], "self-1");
    expect(out.find((t) => t.device_id === "d2")?.disabled).toBe(true);
    expect(out.find((t) => t.device_id === "d2")).toBeDefined(); // 仍列出
    expect(out.find((t) => t.device_id === "d1")?.disabled).toBe(false);
  });

  it("name null → label 兜底 device_id(DeviceTable 同款)", () => {
    const out = quickSendTargets([dev("d1", true, null)], null);
    expect(out[0].label).toBe("d1");
  });

  it("无远端设备(仅本机)→ 空数组(入口隐藏)", () => {
    expect(quickSendTargets([dev("self-1", true)], "self-1")).toEqual([]);
  });
});

describe("quickSendItems(点击契约:恰一次/无确认门/离线禁用)", () => {
  it("在线项 onPick 直调 onQuickSend(deviceId) 恰一次(无确认中介)", () => {
    const onPick = vi.fn();
    const items = quickSendItems(
      quickSendTargets([dev("d1", true), dev("d2", false)], "self-1"),
      OFFLINE_HINT,
      onPick,
    );
    const d1 = items.find((i) => i.key === "qs:d1");
    expect(d1).toBeDefined();
    d1!.onPick!();
    expect(onPick).toHaveBeenCalledTimes(1);
    expect(onPick).toHaveBeenCalledWith("d1");
  });

  it("离线项 disabled + title 提示(灰显非隐藏)", () => {
    const onPick = vi.fn();
    const items = quickSendItems(
      quickSendTargets([dev("d2", false)], "self-1"),
      OFFLINE_HINT,
      onPick,
    );
    expect(items).toHaveLength(1);
    expect(items[0].disabled).toBe(true);
    expect(items[0].title).toBe(OFFLINE_HINT);
    expect(items[0].label).toBe("d2-名");
  });
});

describe("QuickSendFlyout(SSR 冒烟:飞出清单面)", () => {
  it("全部远端设备渲染,本机不出现;离线 disabled+title,在线可点", () => {
    const html = renderToStaticMarkup(
      createElement(QuickSendFlyout, {
        targets: quickSendTargets(
          [dev("self-1", true), dev("d1", true), dev("d2", false)],
          "self-1",
        ),
        offlineHint: OFFLINE_HINT,
        onPick: noop,
        onClose: noop,
        left: 8,
        top: 8,
      }),
    );
    expect(html).toContain("d1-名");
    expect(html).toContain("d2-名"); // 离线仍渲染(灰显非隐藏)
    expect(html).not.toContain("self-1-名"); // 本机剔除
    expect(html).toContain(OFFLINE_HINT); // title 提示
    const d2Tag = html.slice(html.lastIndexOf("<button", html.indexOf("d2-名")), html.indexOf("d2-名"));
    expect(d2Tag).toContain("disabled");
    const d1Tag = html.slice(html.lastIndexOf("<button", html.indexOf("d1-名")), html.indexOf("d1-名"));
    expect(d1Tag).not.toContain("disabled");
    expect(html).toContain('role="menu"');
  });
});

describe("ContextMenu「快速发送到」入口(SSR 冒烟)", () => {
  const quickSend = {
    devices: [dev("self-1", true), dev("d1", true)],
    selfDeviceId: "self-1",
    onPick: noop,
  };

  it("条目菜单 + quickSend prop → 入口呈现(文件/多选行同形)", () => {
    const html = renderMenu({ quickSend });
    expect(html).toContain("快速发送到");
    expect(html).not.toContain("ctxmenu.");
  });

  it("quickSend prop 缺省 → 入口不呈现(作用域门在 App:dir 位本机行)", () => {
    const html = renderMenu({});
    expect(html).not.toContain("快速发送到");
  });

  it("空白菜单 → 入口不呈现(spec 限文件/多选行,非空白处)", () => {
    const html = renderMenu({ menu: blankMenu, quickSend });
    expect(html).not.toContain("快速发送到");
  });

  it("无远端设备(仅本机)→ 入口不呈现", () => {
    const html = renderMenu({
      quickSend: { devices: [dev("self-1", true)], selfDeviceId: "self-1", onPick: noop },
    });
    expect(html).not.toContain("快速发送到");
  });
});
