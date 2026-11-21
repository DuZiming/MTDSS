/// CSP 快照(V4-1 T20 控制器裁定①):ui/src-tauri/tauri.conf.json 的
/// csp/devCsp 必须放行回环口与预览组件运行面——
/// - connect-src `http://127.0.0.1:*`:面板/查看器 fetch→blob(previewer
///   bundle 内 fetch 全走回环 URL);
/// - img-src `http://127.0.0.1:*`:`<img>` 直连面(缩略图/占位降级);
/// - worker-src `blob:`:pptx 解析 worker 以 blob URL 创建(T18 交接挂账
///   ——本仓 pdf.js 主线程假工零 worker,worker-src 是 pptx 独有活约束;
///   缺放行 → pptx 全端「unsupported+officeWorkerFailed」降级);
/// - media-src `blob: http://127.0.0.1:*`:T21 `<video>`/`<audio>` 备。
/// 整串快照 = 改 CSP 必有意翻快照(golden 同纪律);指令断言 = 语义自读。
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

interface TauriConf {
  app: { security: { csp: string; devCsp: string } };
}

const conf = JSON.parse(
  readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"),
) as TauriConf;

/// CSP 串 → 指令源表(小写指令名匹配;无该指令 = 空表)。
function directiveOf(csp: string, name: string): string[] {
  for (const part of csp.split(";")) {
    const [d, ...srcs] = part.trim().split(/\s+/);
    if (d === name) return srcs;
  }
  return [];
}

describe.each([{ key: "csp" as const }, { key: "devCsp" as const }])(
  "tauri.conf.json $key(裁定①白名单)",
  ({ key }) => {
    it("回环口/worker/media 指令齐备", () => {
      const csp = conf.app.security[key];
      expect(directiveOf(csp, "connect-src"), "connect-src 回环口").toContain(
        "http://127.0.0.1:*",
      );
      expect(directiveOf(csp, "img-src"), "img-src 回环口").toContain(
        "http://127.0.0.1:*",
      );
      expect(directiveOf(csp, "worker-src"), "worker-src blob(pptx,T18 交接)").toContain(
        "blob:",
      );
      expect(directiveOf(csp, "media-src"), "media-src blob(T21 备)").toContain("blob:");
      expect(directiveOf(csp, "media-src"), "media-src 回环口(T21 备)").toContain(
        "http://127.0.0.1:*",
      );
    });
  },
);

describe("tauri.conf.json CSP 整串快照(改 CSP 必有意翻快照)", () => {
  it("csp(生产)", () => {
    expect(conf.app.security.csp).toBe(
      "default-src 'self'; img-src 'self' data: http://127.0.0.1:*; style-src 'self' 'unsafe-inline'; " +
        "connect-src 'self' ipc: http://ipc.localhost http://127.0.0.1:*; " +
        "worker-src blob:; media-src blob: http://127.0.0.1:*",
    );
  });

  it("devCsp(开发,保留 vite dev 面)", () => {
    expect(conf.app.security.devCsp).toBe(
      "default-src 'self'; img-src 'self' data: http://127.0.0.1:*; style-src 'self' 'unsafe-inline'; " +
        "connect-src 'self' ws://localhost:1421 http://localhost:1420 http://127.0.0.1:*; " +
        "worker-src blob:; media-src blob: http://127.0.0.1:*",
    );
  });
});
