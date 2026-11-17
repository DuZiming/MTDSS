/// 全局剪贴板快捷键(Ctrl+X/C/V)拦防判定(纯函数,v3p6 T13 自 App.tsx
/// 键盘效应抽出,与 Del 键效应的 modalBusy 门同族)。
///
/// 背景(实地报回归):旧处理器在「空选」「非目录位」两类分支不加
/// preventDefault 直接早退,浏览器默认存活——壳面文字被选中/整页复制
/// (「Ctrl+C 误全选界面文字」),快捷键在这些位表现为「不生效」。判定
/// 收敛为一处后,X/C 在守卫之外**一律拦**(有选走应用侧复制/剪切,空选
/// 仅吞默认),V 维持既有粘贴门不动。
///
/// 判定语义:
/// - 非 Ctrl/Cmd 或非 X/C/V:零波及(false,其余按键不进本门)。
/// - 输入框聚焦(inInput):放行(false)——原生复制/剪切/粘贴语义,
///   既有豁免逐字保留。
/// - 模态占线(modalBusy):放行(false)——镜像既有早退语义(占线位
///   历来不加 preventDefault,评审裁定维持:占线时壳面已全局
///   user-select:none,浏览器默认复制自然落空,改拦属行为变更不属本任务)。
/// - V(粘贴):dir/favdir/fav 位拦截(App 层进粘贴管线);devices/favs
///   位照旧放行(无粘贴目标,既有行为)。V 分派不经选择集,空选同样进
///   (fav 位空选粘贴 = 清内部剪贴板语义,App 层既有)。
/// - X/C(剪切/复制):守卫后**一律拦截**——dir/favdir/fav 位有选走
///   应用侧管线;空选与 devices/favs 位仅吞浏览器默认(无侧效应,App 层
///   判空/判位早退)。选择集**不入判定**:有选/空选拦截同值,选择集只
///   决定 App 层侧效应分派,故不出现在上下文中。

import type { Loc } from "./nav";

export type ClipKeyViewKind = Loc["kind"];

/// 判定上下文:调用点(App 键盘效应)从事件/状态派生后传入,本函数不触
/// DOM 不读状态。
export interface ClipKeyContext {
  /// 焦点落在输入框/文本域/contenteditable(原生编辑面,放行)。
  inInput: boolean;
  /// 任一模态占线(modalBusy 结果,与 Del 键同一守卫)。
  modalBusy: boolean;
  /// 当前视图位(nav.Loc.kind 原样)。
  viewKind: ClipKeyViewKind;
}

/// 键盘事件切片(仅消费修饰键与键名;调用点直传原生 KeyboardEvent 即可,
/// 结构化子集以便纯函数单测)。
export type ClipKeyEvent = Pick<KeyboardEvent, "ctrlKey" | "metaKey" | "key">;

/// 返回 true = 调用点必须 preventDefault(应用侧效应或仅吞浏览器默认);
/// 返回 false = 完全放行(不拦、不产生侧效应)。
export function shouldInterceptClipboardKey(
  e: ClipKeyEvent,
  ctx: ClipKeyContext,
): boolean {
  if (!e.ctrlKey && !e.metaKey) return false;
  const k = e.key.toLowerCase();
  if (k !== "x" && k !== "c" && k !== "v") return false;
  // 输入面与模态占线:镜像既有早退语义(见模块文档),放行不拦。
  if (ctx.inInput || ctx.modalBusy) return false;
  if (k === "v") {
    // 粘贴语义不变:仅目录三位进,其余位既有放行。
    return ctx.viewKind === "dir" || ctx.viewKind === "favdir" || ctx.viewKind === "fav";
  }
  // X/C:守卫后一律拦(误全选回归修复),侧效应由 App 层按位/选择分派。
  return true;
}
