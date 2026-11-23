// @vitest-environment happy-dom
/// 弹出独立窗 boot 消费形钉面(V4-1 T20 fix round 1 Important #1,Rust→窗向
/// 的第四段缝):preview_popout 经 eval 注入 `window.__MTDSS_PREVIEW_BOOT__`
/// = <boot JSON>(popout.rs 线形),本文件钉窗内消费侧——previewer-main.ts
/// 读 boot.req/boot.handle:在场 = mountPreview 收同一 req 对象 + 关窗收口
/// 接线;缺席 = 「缺少预览参数」防御面零装配。
///
/// field r12(2026-11-23 多开重构)关窗语义:Esc/X=真销毁——Esc=win.close;
/// CloseRequested 不再 preventDefault(放行原生销毁),收口=销毁挂载+
/// previewClose 释句柄(IPC 已派发,窗亡不碍)。r10 离屏停放(setPosition)
/// 断言为「不得再出现」。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mountPreviewMock = vi.fn();
const closeMock = vi.fn().mockResolvedValue(undefined);
const destroyWinMock = vi.fn().mockResolvedValue(undefined);
const setPositionMock = vi.fn().mockResolvedValue(undefined);
const hideMock = vi.fn().mockResolvedValue(undefined);
const onCloseRequestedMock = vi.fn().mockResolvedValue(() => {});
const previewCloseMock = vi.fn().mockResolvedValue(undefined);

vi.mock("@mtdss/previewer", () => ({
  mountPreview: (...args: unknown[]) => mountPreviewMock(...args),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    close: (...args: unknown[]) => closeMock(...args),
    destroy: (...args: unknown[]) => destroyWinMock(...args),
    hide: (...args: unknown[]) => hideMock(...args),
    setPosition: (...args: unknown[]) => setPositionMock(...args),
    onCloseRequested: (...args: unknown[]) => onCloseRequestedMock(...args),
  }),
}));
vi.mock("./lib/daemon", () => ({
  previewClose: (...args: unknown[]) => previewCloseMock(...args),
}));

import type { PreviewRequest } from "@mtdss/previewer";

const REQ: PreviewRequest = {
  kind: "unsupported",
  urls: { raw: "http://127.0.0.1:45678/f/h-1/raw?t=tok" },
  meta: { name: "包.zip", size: 7 },
};

async function bootWindow(boot: { req: PreviewRequest; handle: string } | null): Promise<HTMLElement> {
  vi.resetModules();
  document.body.innerHTML = "";
  const host = document.createElement("div");
  host.id = "root";
  document.body.appendChild(host);
  if (boot != null) {
    (window as unknown as { __MTDSS_PREVIEW_BOOT__?: unknown }).__MTDSS_PREVIEW_BOOT__ = boot;
  } else {
    delete (window as unknown as { __MTDSS_PREVIEW_BOOT__?: unknown }).__MTDSS_PREVIEW_BOOT__;
  }
  if (boot != null) {
    await import("./previewer-main");
  } else {
    // boot 缺席 = 模块顶层 await 永续轮询(import promise 永不落)——
    // 这正是被测行为:race 一个短窗让断言先行,import 悬置由 vitest 清场。
    await Promise.race([
      import("./previewer-main"),
      new Promise((resolve) => setTimeout(resolve, 250)),
    ]);
  }
  return host;
}

afterEach(() => {
  // 卸载上一模块实例的 boot-set 监听器(happy-dom window 复用防双触发)
  (window as unknown as { __MTDSS_PREVIEW_TEARDOWN__?: () => void }).__MTDSS_PREVIEW_TEARDOWN__?.();
});

beforeEach(() => {
  mountPreviewMock.mockReset().mockImplementation(() => ({ destroy: destroyMock }));
  closeMock.mockReset().mockResolvedValue(undefined);
  onCloseRequestedMock.mockReset().mockResolvedValue(() => {});
  hideMock.mockReset().mockResolvedValue(undefined);
  setPositionMock.mockReset().mockResolvedValue(undefined);
  previewCloseMock.mockReset().mockResolvedValue(undefined);
});

const destroyMock = vi.fn();

describe("previewer-main(boot 消费形,四段缝第四段;r12 关窗=preventDefault+destroy)", () => {
  it("boot 在场:mountPreview 收 boot.req;Esc=destroy 真销毁;X=preventDefault+destroy+收口句柄", async () => {
    const host = await bootWindow({ req: REQ, handle: "h-9" });
    expect(mountPreviewMock).toHaveBeenCalledTimes(1);
    expect(mountPreviewMock.mock.calls[0][0]).toBe(host); // 挂载点 = #root
    expect(mountPreviewMock.mock.calls[0][1]).toBe(REQ); // req 原样透传(消费 boot.req)
    expect(destroyMock).not.toHaveBeenCalled();
    // Esc = destroy 强销毁(close() 与 CloseRequested 互喂死循环,r12 定形)
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await Promise.resolve();
    expect(destroyWinMock).toHaveBeenCalledTimes(1);
    expect(closeMock).not.toHaveBeenCalled();
    expect(setPositionMock).not.toHaveBeenCalled(); // r10 离屏停放退役
    expect(hideMock).not.toHaveBeenCalled();
    expect(destroyMock).toHaveBeenCalledTimes(1); // 收口链:销毁挂载+释句柄
    expect(previewCloseMock).toHaveBeenCalledWith("h-9");
    // X 关:注册了 CloseRequested;handler 吞原生回合(preventDefault)+
    // destroy 强销毁 + 同面收口
    expect(onCloseRequestedMock).toHaveBeenCalledTimes(1);
    const handler = onCloseRequestedMock.mock.calls[0][0] as (e: {
      preventDefault: () => void;
    }) => void;
    const event = { preventDefault: vi.fn() };
    handler(event);
    await Promise.resolve();
    expect(event.preventDefault).toHaveBeenCalledTimes(1);
    expect(destroyWinMock).toHaveBeenCalledTimes(2);
    // 幂等面:Esc 已收口(current=null),二次关窗不再重复销毁挂载/释句柄
    expect(destroyMock).toHaveBeenCalledTimes(1);
    expect(previewCloseMock).toHaveBeenCalledTimes(1);
    // 非Esc按键零动作
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter" }));
    expect(destroyWinMock).toHaveBeenCalledTimes(2);
  });

  it("boot 缺席:轮询永续等待;boot-set 事件后补挂载", async () => {
    const host = await bootWindow(null);
    expect(mountPreviewMock).not.toHaveBeenCalled();
    expect(hideMock).not.toHaveBeenCalled();
    expect(setPositionMock).not.toHaveBeenCalled();
    expect(closeMock).not.toHaveBeenCalled();
    expect(onCloseRequestedMock).not.toHaveBeenCalled();
    expect(previewCloseMock).not.toHaveBeenCalled();
    expect(host.textContent).not.toContain("缺少预览参数");
    // 后补:boot-set 事件到达即挂(eval 注入派事件;首挂后事件=换面)
    (window as unknown as { __MTDSS_PREVIEW_BOOT__?: unknown }).__MTDSS_PREVIEW_BOOT__ = {
      req: REQ,
      handle: "h-late",
    };
    window.dispatchEvent(new Event("mtdss-boot-set"));
    expect(mountPreviewMock).toHaveBeenCalledTimes(1);
    expect(mountPreviewMock.mock.calls[0][1]).toBe(REQ);
  }, 8000);
});
