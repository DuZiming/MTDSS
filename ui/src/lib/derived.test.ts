import { describe, expect, it } from "vitest";

import { codePointCmp, type Cmp } from "./collate";
import { onlineText, sortDevices, type DeviceInfo } from "./derived";

/// 便捷构造:platform/online 取常规默认值,聚焦排序与文案断言。
function d(device_id: string, name: string | null, online = true): DeviceInfo {
  return { device_id, name, platform: "windows", online };
}

describe("sortDevices", () => {
  // 本组断言的期望序均为 UTF-16 码元序(U+53F0(台)< U+7B14(笔),故
  // 「台式机」恒排在「笔记本」之前)——因此注入 codePointCmp:默认 zhCmp()
  // 在真实环境按拼音 collation(笔记本 bǐ < 台式机 tái),序不同;注入后
  // 断言跨环境(locale)确定。
  it("本机置顶(即使原始序在中间),其余按名称码元序升序", () => {
    const input = [d("dev-z", "台式机"), d("dev-me", "我的本机"), d("dev-a", "笔记本")];
    const out = sortDevices(input, "dev-me", codePointCmp);
    expect(out.map((x) => x.device_id)).toEqual(["dev-me", "dev-z", "dev-a"]);
  });

  it("本机已是第一时仍在第一,不重复移动", () => {
    const input = [d("dev-me", "我的本机"), d("dev-z", "台式机"), d("dev-a", "笔记本")];
    const out = sortDevices(input, "dev-me", codePointCmp);
    expect(out.map((x) => x.device_id)).toEqual(["dev-me", "dev-z", "dev-a"]);
  });

  it("同名设备保序:排序前后相对位置不变(稳定序)", () => {
    const first = d("dev-1", "同名");
    const second = d("dev-2", "同名");
    const out = sortDevices([second, first, d("dev-me", "我的本机")], "dev-me", codePointCmp);
    expect(out.map((x) => x.device_id)).toEqual(["dev-me", "dev-2", "dev-1"]);
  });

  it("无本机(selfDeviceId 为 null)时全部按名称码元序升序", () => {
    const input = [d("dev-z", "台式机"), d("dev-a", "笔记本")];
    const out = sortDevices(input, null, codePointCmp);
    expect(out.map((x) => x.device_id)).toEqual(["dev-z", "dev-a"]);
  });

  it("selfDeviceId 不在列表中时等同无本机", () => {
    const input = [d("dev-z", "台式机"), d("dev-a", "笔记本")];
    const out = sortDevices(input, "dev-missing", codePointCmp);
    expect(out.map((x) => x.device_id)).toEqual(["dev-z", "dev-a"]);
  });

  it("本机离线也置顶;其余在线状态不影响排序", () => {
    const input = [d("dev-a", "笔记本", true), d("dev-me", "我的本机", false)];
    const out = sortDevices(input, "dev-me", codePointCmp);
    expect(out.map((x) => x.device_id)).toEqual(["dev-me", "dev-a"]);
  });

  it("注入的 cmp 被实际采用(逆序 spy → 名称降序,且确实被调用)", () => {
    let calls = 0;
    const reversed: Cmp = (a, b) => {
      calls += 1;
      return codePointCmp(b, a);
    };
    const input = [d("dev-b", "banana"), d("dev-a", "apple"), d("dev-c", "cherry")];
    const out = sortDevices(input, null, reversed);
    expect(out.map((x) => x.name)).toEqual(["cherry", "banana", "apple"]);
    expect(calls).toBeGreaterThan(0);
  });

  it("默认参数(zhCmp)下 ASCII 名称仍字典序、本机仍置顶(任意正确 collator 皆然)", () => {
    const input = [d("dev-b", "banana"), d("dev-me", "self"), d("dev-a", "apple")];
    const out = sortDevices(input, "dev-me");
    expect(out.map((x) => x.device_id)).toEqual(["dev-me", "dev-a", "dev-b"]);
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
