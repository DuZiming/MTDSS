/// 对话框焦点圈 hook(Task 7):open 期间把 Tab 圈在 ref 容器内。
/// open=true:记录打开前的 `document.activeElement`,聚焦首个可聚焦项
/// (initialFocusIndex);ref 容器上拦 Tab——焦点将越界(首部后退 /
/// 末部前进 / 容器无可聚焦项)才 `preventDefault` + 按 nextFocusIndex
/// 环绕聚焦,圈内移动交给浏览器原生行为;open=false / 卸载还原焦点。
/// Esc 与遮罩点击语义归对话框组件,不在此处。
import { useEffect, useRef, type RefObject } from "react";

import { focusablesIn, initialFocusIndex, nextFocusIndex } from "./focustrap";

export function useFocusTrap(open: boolean): RefObject<HTMLDivElement | null> {
  const ref = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!open) return;
    const root = ref.current;
    if (root == null) return; // open 时容器必已挂载;防御兜底
    const restoreTo = document.activeElement;
    const focusables = focusablesIn(root);
    focusables[initialFocusIndex(focusables.length)]?.focus();

    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Tab") return;
      const els = focusablesIn(root);
      const current = els.indexOf(document.activeElement as HTMLElement);
      // 越界判定:Shift 在首部(或越界 -1)之后退、前进在末部;空表时
      // current 恒 -1,双向都算越界(吞键,防焦点逃出模态)。
      const leaving = e.shiftKey ? current <= 0 : current >= els.length - 1;
      if (!leaving) return;
      e.preventDefault();
      els[nextFocusIndex(els.length, current, e.shiftKey)]?.focus();
    };
    root.addEventListener("keydown", onKey);
    return () => {
      root.removeEventListener("keydown", onKey);
      if (restoreTo instanceof HTMLElement) restoreTo.focus();
    };
  }, [open]);
  return ref;
}
