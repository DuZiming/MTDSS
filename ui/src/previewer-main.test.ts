// @vitest-environment happy-dom
/// 弹出独立窗 boot 消费形钉面(V4-1 T20 fix round 1 Important #1,Rust→窗向
/// 的第四段缝):preview_popout 经 initialization_script 注入
/// `window.__MTDSS_PREVIEW_BOOT__ = <boot JSON>`(popout.rs 线形),本文件钉
/// 窗内消费侧——previewer-main.ts 读 boot.req/boot.handle:在场 = mountPreview
/// 收同一 req 对象 + Esc 关窗接线;缺席 = 「缺少预览参数」防御面零装配。
/// 模块顶层自执行,vi.resetModules + 动态 import 逐例重跑;Tauri window 面
/// 与 previewer 包 vi.mock(jsdom/happy-dom 无 internals)。
import { beforeEach, describe, expect, it, vi } from "vitest";

const mountPreviewMock = vi.fn();
const closeMock = vi.fn().mockResolvedValue(undefined);
const onCloseRequestedMock = vi.fn().mockResolvedValue(() => {});
const previewCloseMock = vi.fn().mockResolvedValue(undefined);

vi.mock("@mtdss/previewer", () => ({
  mountPreview: (...args: unknown[]) => mountPreviewMock(...args),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    close: (...args: unknown[]) => closeMock(...args),
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
  await import("./previewer-main");
  return host;
}

beforeEach(() => {
  mountPreviewMock.mockReset().mockImplementation(() => ({ destroy: () => {} }));
  closeMock.mockReset().mockResolvedValue(undefined);
  onCloseRequestedMock.mockReset().mockResolvedValue(() => {});
  previewCloseMock.mockReset().mockResolvedValue(undefined);
});

describe("previewer-main(boot 消费形,四段缝第四段)", () => {
  it("boot 在场:mountPreview 收 __MTDSS_PREVIEW_BOOT__.req 同一对象;Esc 关窗接线在", async () => {
    const host = await bootWindow({ req: REQ, handle: "h-9" });
    expect(mountPreviewMock).toHaveBeenCalledTimes(1);
    expect(mountPreviewMock.mock.calls[0][0]).toBe(host); // 挂载点 = #root
    expect(mountPreviewMock.mock.calls[0][1]).toBe(REQ); // req 原样透传(消费 boot.req)
    // Esc = 关独立窗(mockup 确认点⑥)
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(closeMock).toHaveBeenCalledTimes(1);
    // 关窗收口接线消费 boot.handle(previewClose("h-9"))
    expect(onCloseRequestedMock).toHaveBeenCalledTimes(1);
    const handler = onCloseRequestedMock.mock.calls[0][0] as () => void;
    handler();
    await Promise.resolve();
    expect(previewCloseMock).toHaveBeenCalledWith("h-9");
  });

  it("boot 缺席(直起窗/注入失败):防御文案零装配零接线", async () => {
    const host = await bootWindow(null);
    expect(mountPreviewMock).not.toHaveBeenCalled();
    expect(closeMock).not.toHaveBeenCalled();
    expect(onCloseRequestedMock).not.toHaveBeenCalled();
    expect(previewCloseMock).not.toHaveBeenCalled();
    expect(host.textContent).toContain("缺少预览参数");
  });
});
