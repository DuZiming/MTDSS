/// 弹出独立窗调用形钉面(V4-1 T20 fix round 1 Important #1,TS→Rust 向):
/// preview_popout 载荷是跨语言四段缝的第一段——App 装配(previewPopoutPayload
/// 纯函数)→ daemon.ts invoke("preview_popout",{payload}) → Rust
/// payload.get("title")/.get("boot") → 窗内 __MTDSS_PREVIEW_BOOT__。本文件
/// 钉前两段:装配函数逐字段(title=文件名/boot.req=buildPreviewRequest 同源/
/// boot.handle=句柄)+ invoke 线形(命令名逐字+payload 原样透传)。任一侧
/// 改名,此文件先红,不再静默落到「缺少预览参数」防御面。
import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import type { FsEntry } from "./browse";
import type { PreviewOpenInfo, PreviewPopoutPayload } from "./daemon";
import { previewPopout } from "./daemon";
import { buildPreviewRequest, previewPopoutPayload } from "./preview";

const INFO: PreviewOpenInfo = { handle: "h-1", port: 45678, token: "tok" };
const ENTRY: FsEntry = {
  name: "横版照片.jpg",
  path: "C:\\相册\\横版照片.jpg",
  size: 3300000,
  mtime_ms: 1760000000000,
  is_dir: false,
};

beforeEach(() => {
  invokeMock.mockReset().mockResolvedValue(undefined);
});

describe("previewPopoutPayload(装配纯函数,四段缝第一段)", () => {
  it("title=文件名;boot.req=buildPreviewRequest 同源装配;boot.handle=句柄", () => {
    const payload: PreviewPopoutPayload = previewPopoutPayload(ENTRY, INFO);
    expect(payload.title).toBe("横版照片.jpg");
    expect(payload.boot.handle).toBe("h-1");
    expect(payload.boot.req).toEqual(
      buildPreviewRequest(INFO, ENTRY.name, ENTRY.size, `${ENTRY.path}|${ENTRY.mtime_ms}|${ENTRY.size}`));
    // r45:meta.key = 封面持久缓存键(path|mtime|size)随 boot 携
    expect(payload.boot.req.meta.key).toBe(`${ENTRY.path}|${ENTRY.mtime_ms}|${ENTRY.size}`);
    // URL 形随缝语义一并锚(弹窗消费 ?t= 凭据)
    expect(payload.boot.req.urls.raw).toBe("http://127.0.0.1:45678/f/h-1/raw?t=tok");
    expect(payload.boot.req.meta.name).toBe("横版照片.jpg");
  });
});

describe("previewPopout(invoke 线形,四段缝第二段)", () => {
  it("命令名逐字 preview_popout;载荷经 {payload} 单键透传不整形", async () => {
    const payload = previewPopoutPayload(ENTRY, INFO);
    await previewPopout(payload);
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("preview_popout", { payload });
  });
});
