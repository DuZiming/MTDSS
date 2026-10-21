/// 对话框焦点圈纯函数面(Task 7):环绕索引计算与可聚焦元素收集。
/// 副作用/焦点操作在 [`useFocusTrap`](../useFocusTrap);DOM 细节不进
/// node 环境单测(手动冒烟),这里只放可纯测的部分——索引是纯数组
/// 语义,`focusablesIn` 的收集序/禁用过滤以 stub 钉住。

/// Tab 环绕索引:末 + 前进 → 首、首 + Shift → 末(双向环绕);current
/// 越界(-1,焦点在非可聚焦子元素上)时前进视为从首、Shift 视为从末;
/// count ≤ 0 → -1(无可聚焦项,调用方应吞键防焦点逃出对话框)。
export function nextFocusIndex(count: number, current: number, shift: boolean): number {
  if (count <= 0) return -1;
  const last = count - 1;
  if (shift) return current <= 0 ? last : current - 1;
  return current >= last ? 0 : current + 1;
}

/// 初始聚焦索引:有可聚焦项 → 0(首个可聚焦项,对话框惯例是「取消」
/// 等安全键);没有 → -1(调用方不聚焦)。
export function initialFocusIndex(count: number): number {
  return count > 0 ? 0 : -1;
}

/// 可聚焦选择器面:button/[href]/input/select/textarea 与显式 tabindex
/// (排除 tabindex="-1" 的程序聚焦位)。
const FOCUSABLE_SELECTOR =
  'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])';

/// root 子树内的可聚焦元素(document order,即 querySelectorAll 序),
/// 再滤禁用项(HTMLMediaElement/表单族以外无 disabled 字段,不受影响)。
export function focusablesIn(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
    (el) => !(el as HTMLButtonElement).disabled,
  );
}
