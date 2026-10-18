import { describe, expect, it } from "vitest";

import { onlineText, sortDevices, type DeviceInfo } from "./derived";

/// 便捷构造:platform/online 取常规默认值,聚焦排序与文案断言。
function d(device_id: string, name: string | null, online = true): DeviceInfo {
  return { device_id, name, platform: "windows", online };
}

describe("sortDevices", () => {
  it("本机置顶(即使原始序在中间),其余按名称升序", () => {
    const input = [d("dev-z", "台式机"), d("dev-me", "我的本机"), d("dev-a", "笔记本")];
    const out = sortDevices(input, "dev-me");
    expect(out.map((x) => x.device_id)).toEqual(["dev-me", "dev-a", "dev-z"]);
  });

  it("本机已是第一时仍在第一,不重复移动", () => {
    const input = [d("dev-me", "我的本机"), d("dev-z", "台式机"), d("dev-a", "笔记本")];
    const out = sortDevices(input, "dev-me");
    expect(out.map((x) => x.device_id)).toEqual(["dev-me", "dev-a", "dev-z"]);
  });

  it("同名设备保序:排序前后相对位置不变(稳定序)", () => {
    const first = d("dev-1", "同名");
    const second = d("dev-2", "同名");
    const out = sortDevices([second, first, d("dev-me", "我的本机")], "dev-me");
    expect(out.map((x) => x.device_id)).toEqual(["dev-me", "dev-2", "dev-1"]);
  });

  it("无本机(selfDeviceId 为 null)时全部按名称升序", () => {
    const input = [d("dev-z", "台式机"), d("dev-a", "笔记本")];
    const out = sortDevices(input, null);
    expect(out.map((x) => x.device_id)).toEqual(["dev-a", "dev-z"]);
  });

  it("selfDeviceId 不在列表中时等同无本机", () => {
    const input = [d("dev-z", "台式机"), d("dev-a", "笔记本")];
    const out = sortDevices(input, "dev-missing");
    expect(out.map((x) => x.device_id)).toEqual(["dev-a", "dev-z"]);
  });

  it("本机离线也置顶;其余在线状态不影响排序", () => {
    const input = [d("dev-a", "笔记本", true), d("dev-me", "我的本机", false)];
    const out = sortDevices(input, "dev-me");
    expect(out.map((x) => x.device_id)).toEqual(["dev-me", "dev-a"]);
  });
});

describe("onlineText", () => {
  it("未登录 → 「未登录」", () => {
    expect(onlineText({ logged_in: false, online: false })).toBe("未登录");
    expect(onlineText({ logged_in: false, online: true })).toBe("未登录");
  });

  it("已登录未在线 → 「离线」", () => {
    expect(onlineText({ logged_in: true, online: false })).toBe("离线");
  });

  it("已登录且在线 → 「在线」", () => {
    expect(onlineText({ logged_in: true, online: true })).toBe("在线");
  });
});
