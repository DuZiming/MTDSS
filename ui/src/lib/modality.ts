/// 模态占线判定(全局快捷键 / OS 拖入受理门)。
///
/// 背景(Task 12 评审 Important #1):Del 键处理器曾只挡 右键菜单/删除确认,
/// 漏了冲突对话框——`pendingConflict` 打开时按 Del 会在 aria-modal 遮罩之下
/// 对当前选择发起删除(还可能把 force 删除确认叠在冲突对话框上)。这与
/// Task 12 开工前的 MUST-FIX(confirmDel 快照)是同一类「模态逃逸」。
///
/// 谓词用**必填命名字段**而非可变参数:调用点必须逐个点名模态位,将来
/// 新增模态位时,所有调用点漏传即 TypeScript 编译错——把这类「某一处忘
/// 了加守卫」的回归钉死在编译期。
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
  /// 新终端审批弹窗打开态(V3-1 Task 10;null = 关闭):受信终端侧主窗
  /// aria-modal,同意/拒绝期间 Del/Ctrl 快捷键与拖入照例不受理。
  approvalModal: unknown;
  /// 新建/重命名快捷目录对话框打开态(V3-3 Task 8;布尔真值语义同
  /// updatePage):名称输入中按 Del 会落到遮罩之下的背景选择,同属
  /// 「模态逃逸」,一并占线。
  favName: boolean | null;
  /// 快捷位多组删除确认梯(V3-3 Task 11;对象|null):dir 位 confirmDel
  /// 的 fav 位独立实例,同属占线位。
  favDel: unknown;
  /// 「添加到快捷目录」picker(V3-3 Task 11;对象|null):清单点选期间
  /// 遮罩下的背景选择照例不受理。
  favPick: unknown;
  /// 快捷成员选择层(V3-3 Task 12;对象|null):拖放/粘贴进多成员快捷的
  /// 「存入哪个成员」弹层,点选期间快捷键/拖入照例不受理。
  favMemberPick: unknown;
  /// 侧栏快捷行右键菜单(V3-3 Task 13;Favorite|null = 从 store 现值解析
  /// 的目标本体,他端删除快捷后解析为 null,占线自动解除——memberPickFav
  /// 同款语义;若传原始暂存 id,菜单开着删快捷会卡占线)。
  favMenu: unknown;
  /// 快捷目录管理屏(V3-3 Task 13;Favorite|null 同上解析语义)。
  favManage: unknown;
  /// 「删除快捷目录」确认梯(V3-3 Task 13;string|null = 待删快捷 id)。
  favDelFor: unknown;
}

/// 任一模态打开即 true:全局快捷键(Del / Ctrl+X/C/V)与外部拖入此时
/// 一律不受理——遮罩之下的背景操作必须显式先关模态。
export function modalBusy(m: ModalStates): boolean {
  return (
    m.ctxMenu != null ||
    m.confirmDel != null ||
    m.pendingConflict != null ||
    m.updatePage === true ||
    m.approvalModal != null ||
    m.favName === true ||
    m.favDel != null ||
    m.favPick != null ||
    m.favMemberPick != null ||
    m.favMenu != null ||
    m.favManage != null ||
    m.favDelFor != null
  );
}
