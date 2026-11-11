import { describe, expect, it } from "vitest";

import {
  shouldInterceptClipboardKey,
  type ClipKeyContext,
} from "./clipkeys";

/// 便捷构造:空闲基线(非输入框、无模态、目录位)。
function baseCtx(over: Partial<ClipKeyContext> = {}): ClipKeyContext {
  return { inInput: false, modalBusy: false, viewKind: "dir", ...over };
}

/// 便捷构造:Ctrl(+修饰)键事件(key 统一小写面,大写走 CapsLock 用例)。
function ctrl(key: string, over: { ctrlKey?: boolean; metaKey?: boolean } = {}) {
  return { ctrlKey: true, metaKey: false, key, ...over };
}

/// v3p6 T13:Ctrl+X/C/V 全局拦防判定(自 App.tsx 键盘效应抽出的纯函数)。
/// 回归主案 = 实地报「Ctrl+C 误全选界面文字」:空选时旧处理器不加
/// preventDefault 直接早退,浏览器默认(壳面文字选中/复制)存活。
describe("shouldInterceptClipboardKey(全局剪贴板快捷键拦防判定)", () => {
  it("回归主案:目录位空选 + Ctrl+C → 拦截(preventDefault,吞浏览器默认)", () => {
    expect(shouldInterceptClipboardKey(ctrl("c"), baseCtx())).toBe(true);
  });

  it("目录位空选 + Ctrl+X → 同拦(X 与 C 同规:空选仅吞默认,无侧效应)", () => {
    expect(shouldInterceptClipboardKey(ctrl("x"), baseCtx())).toBe(true);
  });

  it("目录位有选 + Ctrl+C → 拦截(App 层走 clipFromSelection,既有行为)", () => {
    // 判定面与空选同值:选择集只决定 App 层侧效应分派,不入判定
    expect(shouldInterceptClipboardKey(ctrl("c"), baseCtx())).toBe(true);
    expect(shouldInterceptClipboardKey(ctrl("x"), baseCtx())).toBe(true);
  });

  it("非目录位(devices 设备层 / favs 快捷着陆位)+ Ctrl+C/X → 拦截", () => {
    // 旧处理器在这些位直接早退不拦,壳面默认复制存活 = 「快捷键不生效」面
    expect(
      shouldInterceptClipboardKey(ctrl("c"), baseCtx({ viewKind: "devices" })),
    ).toBe(true);
    expect(
      shouldInterceptClipboardKey(ctrl("x"), baseCtx({ viewKind: "favs" })),
    ).toBe(true);
  });

  it("fav 位(快捷视图)空选 + Ctrl+C → 拦截;有选同值(侧效应走 copyCutMixed)", () => {
    expect(
      shouldInterceptClipboardKey(ctrl("c"), baseCtx({ viewKind: "fav" })),
    ).toBe(true);
    expect(
      shouldInterceptClipboardKey(ctrl("x"), baseCtx({ viewKind: "fav" })),
    ).toBe(true);
  });

  it("输入框聚焦 → 一律不拦(原生复制/剪切/粘贴语义放行,既有豁免)", () => {
    expect(
      shouldInterceptClipboardKey(
        ctrl("c"),
        baseCtx({ inInput: true }),
      ),
    ).toBe(false);
    expect(
      shouldInterceptClipboardKey(
        ctrl("x"),
        baseCtx({ inInput: true }),
      ),
    ).toBe(false);
    expect(
      shouldInterceptClipboardKey(
        ctrl("v"),
        baseCtx({ inInput: true }),
      ),
    ).toBe(false);
    // 非目录位输入框同理(devices 位搜索框)
    expect(
      shouldInterceptClipboardKey(
        ctrl("c"),
        baseCtx({ inInput: true, viewKind: "devices" }),
      ),
    ).toBe(false);
  });

  it("模态占线 → 不拦(镜像既有早退语义:占线位不加 preventDefault)", () => {
    // 语义存证(评审裁定):模态遮罩下浏览器默认放行维持现状——全局
    // user-select:none(T13 CSS 半场)已使壳面无文本可选,默认复制自然
    // 落空;若改占线即拦属行为变更,不属本任务。
    expect(
      shouldInterceptClipboardKey(ctrl("c"), baseCtx({ modalBusy: true })),
    ).toBe(false);
    expect(
      shouldInterceptClipboardKey(
        ctrl("v"),
        baseCtx({ modalBusy: true, viewKind: "fav" }),
      ),
    ).toBe(false);
  });

  it("Ctrl+V:dir/favdir/fav 位 → 拦截(App 层进粘贴);非目录位 → 不拦(既有)", () => {
    expect(shouldInterceptClipboardKey(ctrl("v"), baseCtx())).toBe(true);
    expect(
      shouldInterceptClipboardKey(ctrl("v"), baseCtx({ viewKind: "favdir" })),
    ).toBe(true);
    expect(
      shouldInterceptClipboardKey(ctrl("v"), baseCtx({ viewKind: "fav" })),
    ).toBe(true);
    // V 粘贴语义不变:非目录位无粘贴目标,旧处理器即早退,照旧放行
    expect(
      shouldInterceptClipboardKey(ctrl("v"), baseCtx({ viewKind: "devices" })),
    ).toBe(false);
    expect(
      shouldInterceptClipboardKey(ctrl("v"), baseCtx({ viewKind: "favs" })),
    ).toBe(false);
  });

  it("favdir 位(命名空间目录)空选 + Ctrl+C/X → 拦截", () => {
    expect(
      shouldInterceptClipboardKey(ctrl("c"), baseCtx({ viewKind: "favdir" })),
    ).toBe(true);
    expect(
      shouldInterceptClipboardKey(ctrl("x"), baseCtx({ viewKind: "favdir" })),
    ).toBe(true);
  });

  it("非 Ctrl/Cmd 或非 X/C/V 键 → 不拦(其余按键零波及)", () => {
    expect(shouldInterceptClipboardKey(ctrl("a"), baseCtx())).toBe(false);
    expect(shouldInterceptClipboardKey(ctrl("Delete"), baseCtx())).toBe(false);
    expect(
      shouldInterceptClipboardKey(
        { ctrlKey: false, metaKey: false, key: "c" },
        baseCtx(),
      ),
    ).toBe(false);
    // Shift/Ctrl 组合非本族键不误拦
    expect(
      shouldInterceptClipboardKey(
        { ctrlKey: false, metaKey: false, key: "x" },
        baseCtx(),
      ),
    ).toBe(false);
  });

  it("大写 C/X/V(CapsLock)与 Cmd(metaKey,macOS 面)同拦", () => {
    expect(shouldInterceptClipboardKey(ctrl("C"), baseCtx())).toBe(true);
    expect(shouldInterceptClipboardKey(ctrl("V"), baseCtx())).toBe(true);
    expect(
      shouldInterceptClipboardKey(
        { ctrlKey: false, metaKey: true, key: "c" },
        baseCtx(),
      ),
    ).toBe(true);
    expect(
      shouldInterceptClipboardKey(
        { ctrlKey: false, metaKey: true, key: "x" },
        baseCtx({ viewKind: "devices" }),
      ),
    ).toBe(true);
  });
});
