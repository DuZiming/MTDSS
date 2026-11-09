/// V3-4 §5:Windows 设置面(工具栏齿轮 → SettingsDialog)+ 四项 +
/// 顺手票的测试面。分组:
/// ①门默认关(V34_DEFAULTS = 桌面旧行为零回归);
/// ②信任模式开 → roots.set 载荷 = 收件箱 + 所选目录(trustRootsPayload
///   纯函数;关 = 空集直通);
/// ③显示隐藏过滤(filterHiddenEntries:开关关滤 dotfile,开全量);
/// ④设备名迁移(登录表单无设备名字段;loginNameArg 修剪/空转 null)。
/// 组件面沿 SSR 冒烟纪律(react-dom/server 纯字符串渲染,不跑 effects;
/// lib/daemon mock 掉——组件本身零 invoke,仅 GuideView 既有登录链 import)。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

vi.mock("../../lib/daemon", () => ({
  loginCancel: vi.fn(),
  loginEvict: vi.fn(),
  loginQrBegin: vi.fn(),
  loginQrStatus: vi.fn(),
  loginRequestCode: vi.fn(),
  loginStatus: vi.fn(),
  loginSubmitCode: vi.fn(),
  loginVerify: vi.fn(),
  pickCaCert: vi.fn(),
  toRpcError: vi.fn(),
}));

import "../../i18n";

import { filterHiddenEntries } from "../../lib/browse";
import { V34_DEFAULTS, loginNameArg, trustRootsPayload, type V34Settings } from "../model";
import SettingsDialog from "../SettingsDialog";
import GuideView from "../../components/GuideView";

describe("设置面默认与信任模式载荷", () => {
  /// ①门默认关:三开关全关 + 信任目录空表 + 无设备名 = 桌面旧行为
  /// (删除入口照旧/浏览全量/授权根 None 直通/登录回落主机名)。
  it("defaults keep legacy desktop behavior", () => {
    const d: V34Settings = V34_DEFAULTS;
    expect(d.allow_remote_delete).toBe(false);
    expect(d.show_hidden).toBe(false);
    expect(d.trust_mode).toBe(false);
    expect(d.trusted_roots).toEqual([]);
    expect(d.device_name).toBeNull();
  });

  /// ②信任模式开 → roots.set 载荷 = 收件箱 + 所选目录(顺序保留;
  /// 收件箱名固定「收件箱」,所选目录名空串 = 守卫回退末段真实目录名;
  /// 全部 fs 源 + 对端可见)。
  it("trust payload is inbox plus selected dirs", () => {
    const roots = trustRootsPayload("C:\\Users\\u\\Documents\\MTDSS", [
      "D:\\媒体库",
      "C:\\代码",
    ]);
    expect(roots).toEqual([
      { name: "收件箱", path: "C:\\Users\\u\\Documents\\MTDSS", source: "fs", visible: true },
      { name: "", path: "D:\\媒体库", source: "fs", visible: true },
      { name: "", path: "C:\\代码", source: "fs", visible: true },
    ]);
  });

  /// ②b:收件箱路径缺失(status 未就绪)→ 载荷只含所选目录,不出空路径
  /// 条目(空路径进守卫必被丢弃,徒增 fail-closed 噪声)。
  it("trust payload omits inbox when recv_dir missing", () => {
    const roots = trustRootsPayload(null, ["C:\\代码"]);
    expect(roots).toEqual([{ name: "", path: "C:\\代码", source: "fs", visible: true }]);
  });

  /// ②c:信任模式开但未选目录 → 载荷恰收件箱一枚(仅收件箱可被浏览);
  /// 信任模式关 = 空集载荷(App 侧直发 `[]`,T1 op 空集 = 守卫直通,
  /// 桌面默认恢复)。
  it("trust on with no dirs is inbox-only; off is empty passthrough", () => {
    expect(trustRootsPayload("C:\\recv", [])).toEqual([
      { name: "收件箱", path: "C:\\recv", source: "fs", visible: true },
    ]);
    expect(trustRootsPayload(null, [])).toEqual([]);
  });
});

describe("显示隐藏过滤", () => {
  const es = [
    { name: "报告.txt", path: "C:\\d\\报告.txt", size: 1, mtime_ms: 0, is_dir: false },
    { name: ".git", path: "C:\\d\\.git", size: 0, mtime_ms: 0, is_dir: true },
    { name: ".env", path: "C:\\d\\.env", size: 1, mtime_ms: 0, is_dir: false },
    { name: "源码", path: "C:\\d\\源码", size: 0, mtime_ms: 0, is_dir: true },
  ];

  /// ③开关关(默认)= 滤除 dotfile(表头/浏览/快捷树同管线)。
  it("hides dotfiles when switch off", () => {
    const out = filterHiddenEntries(es, false);
    expect(out.map((e) => e.name)).toEqual(["报告.txt", "源码"]);
  });

  /// ③b:开关开 = 全量透出(新数组,不改入参)。
  it("shows everything when switch on", () => {
    const out = filterHiddenEntries(es, true);
    expect(out).toHaveLength(4);
    expect(out).not.toBe(es);
  });
});

describe("设备名迁移", () => {
  /// ④登录表单去设备名字段:渲染未登录引导态(SSR)不含设备名行——
  /// 词面与占位符均不出(字段已迁设置面,登录链改读 settings.device_name)。
  it("guide view renders without device name field", () => {
    const html = renderToStaticMarkup(
      createElement(GuideView, { onLoggedIn: () => {}, deviceName: "台式机" }),
    );
    expect(html).not.toContain("设备名");
    expect(html).not.toContain("默认主机名");
    expect(html).toContain("高级选项");
  });

  /// ④b:登录链取名参数(既有语义不变:空 = 传 null → daemon 回落主机名;
  /// 非空去首尾空白)。
  it("login name arg trims and nulls empty", () => {
    expect(loginNameArg(null)).toBeNull();
    expect(loginNameArg(undefined)).toBeNull();
    expect(loginNameArg("")).toBeNull();
    expect(loginNameArg("   ")).toBeNull();
    expect(loginNameArg("  台式机  ")).toBe("台式机");
  });
});

describe("设置对话框", () => {
  /// 组件 SSR 冒烟:关闭态渲染 null;打开态含四项分组词面 + 检查更新 +
  /// 关于(版本随 props 注入)。
  it("renders groups when open, null when closed", () => {
    const closed = renderToStaticMarkup(
      createElement(SettingsDialog, {
        open: false,
        settings: V34_DEFAULTS,
        version: "1.2.1",
        busy: false,
        onPatch: () => {},
        onOpenUpdate: () => {},
        onClose: () => {},
      }),
    );
    expect(closed).toBe("");

    const html = renderToStaticMarkup(
      createElement(SettingsDialog, {
        open: true,
        settings: {
          ...V34_DEFAULTS,
          device_name: "台式机",
          trust_mode: true,
          trusted_roots: ["D:\\媒体库"],
        },
        version: "1.2.1",
        busy: false,
        onPatch: () => {},
        onOpenUpdate: () => {},
        onClose: () => {},
      }),
    );
    expect(html).toContain("设备名");
    expect(html).toContain("台式机");
    expect(html).toContain("显示隐藏文件");
    expect(html).toContain("允许删除远端文件");
    expect(html).toContain("限制本机被浏览范围");
    expect(html).toContain("D:\\媒体库");
    expect(html).toContain("检查更新");
    expect(html).toContain("1.2.1");
  });
});
