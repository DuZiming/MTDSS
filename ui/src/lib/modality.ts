/// 模态占线判定(全局快捷键 / OS 拖入受理门)。
///
/// 背景(Task 12 评审 Important #1):Del 键处理器曾只挡 右键菜单/删除确认,
/// 漏了冲突对话框——`pendingConflict` 打开时按 Del 会在 aria-modal 遮罩之下
/// 对当前选择发起删除(还可能把 force 删除确认叠在冲突对话框上)。这与
/// Task 12 开工前的 MUST-FIX(confirmDel 快照)是同一类「模态逃逸」。
///
/// 谓词用**必填命名字段**而非可变参数:调用点必须逐个点名三个模态位,
/// 将来新增第四个模态时,所有调用点漏传即 TypeScript 编译错——把这类
/// 「某一处忘了加守卫」的回归钉死在编译期。
export interface ModalStates {
  /// 右键菜单打开态(null = 关闭)。
  ctxMenu: unknown;
  /// 删除确认对话框打开态(null = 关闭)。
  confirmDel: unknown;
  /// 冲突对话框打开态(null = 关闭)。
  pendingConflict: unknown;
  /// 更新页覆盖层打开态(Task 15;false/null = 关闭):aria-modal 遮罩全屏
  /// 覆盖,Del/Ctrl 快捷键与拖入在其下操作背景文件是同一类「模态逃逸」。
  /// 语义为**布尔真值**而非 null 语义:App 侧以 `useState(false)` 直传,
  /// 关闭态是 `false`——若按 `!= null` 判定则恒占线(M4b 冒烟实测回归,
  /// 见 modality.test.ts 布尔回归用例)。
  updatePage: boolean | null;
}

/// 任一模态打开即 true:全局快捷键(Del / Ctrl+X/C/V)与外部拖入此时
/// 一律不受理——遮罩之下的背景操作必须显式先关模态。
export function modalBusy(m: ModalStates): boolean {
  return (
    m.ctxMenu != null ||
    m.confirmDel != null ||
    m.pendingConflict != null ||
    m.updatePage === true
  );
}
