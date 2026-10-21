/// 登录链路 invoke 契约测试(fix-wave Defect B):vi.mock 掉 @tauri-apps
/// 运行时,断言桥命令名与参数面逐字——login_request_code 的 ca 透传
/// (daemon 侧持久化到待登录槽,verify 复用)与 pick_ca_cert 取消语义。
import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));

import { loginRequestCode, pickCaCert } from "./daemon";

describe("login invoke 契约(桥命令名与参数面逐字)", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    invokeMock.mockResolvedValue({});
  });

  it("loginRequestCode threads ca path through to the bridge", async () => {
    invokeMock.mockResolvedValue({ code: "123456" });
    await loginRequestCode("https://s:8787", "a@b.c", "C:\\certs\\ca.pem");
    expect(invokeMock).toHaveBeenCalledWith("login_request_code", {
      server: "https://s:8787",
      email: "a@b.c",
      ca: "C:\\certs\\ca.pem",
    });
  });

  it("loginRequestCode without ca sends explicit null(桥 Option → None)", async () => {
    invokeMock.mockResolvedValue({ code: "123456" });
    await loginRequestCode("https://s", "a@b.c");
    expect(invokeMock).toHaveBeenCalledWith("login_request_code", {
      server: "https://s",
      email: "a@b.c",
      ca: null,
    });
  });

  it("pickCaCert returns the picked certificate path", async () => {
    invokeMock.mockResolvedValue({ path: "C:\\certs\\root.crt" });
    await expect(pickCaCert()).resolves.toBe("C:\\certs\\root.crt");
    expect(invokeMock).toHaveBeenCalledWith("pick_ca_cert");
  });

  it("pickCaCert null = user cancel(不放大为错误)", async () => {
    invokeMock.mockResolvedValue({ path: null });
    await expect(pickCaCert()).resolves.toBeNull();
  });
});
