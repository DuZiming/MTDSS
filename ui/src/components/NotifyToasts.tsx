/// 窗内通知回显 toast(Task 13,Task 9 自 TransferCards.tsx 迁出独立成件):
/// event.notify 的窗内瞬时 toast(5s 自动消隐,不堆叠——队列最多 3 条,
/// 超出丢最旧)。OS 系统通知由 daemon 侧自发(notify-rust),此处**只做
/// 窗内回显**,不重复弹系统通知。传输控制错误(App 侧 toastError)与
/// 更新 available 提示(「查看」直达更新页)共用同一队列。
import { useEffect } from "react";

/// 单条 toast 数据(id 由 App 侧单调分配,tone 区分信息/错误色)。
export interface ToastItem {
  id: number;
  title: string;
  body: string;
  tone: "info" | "error";
  /// 可选动作(Task 15):更新 toast 的「查看」直达更新页;点击不自动
  /// 出队(5s TTL 照常),动作闭包由 App 侧装配。
  action?: { label: string; onClick(): void };
}

/// toast 展示时长(窗内回显,规格钉死 5s)。
const TOAST_TTL_MS = 5000;

/// 窗内 toast 栈:每条 5s 自隐(onExpire 挪出队列);不堆叠——App 侧入队
/// 时截尾最多 3 条。位置固定窗底中部。
export function NotifyToasts({
  toasts,
  onExpire,
}: {
  toasts: readonly ToastItem[];
  onExpire(id: number): void;
}) {
  if (toasts.length === 0) return null;
  return (
    <div className="toasts" role="status">
      {toasts.map((t) => (
        <ToastRow key={t.id} toast={t} onExpire={onExpire} />
      ))}
    </div>
  );
}

/// 单条 toast:计时器挂自身 effect,卸载即清(队列截尾移除同样收干净)。
function ToastRow({
  toast,
  onExpire,
}: {
  toast: ToastItem;
  onExpire(id: number): void;
}) {
  useEffect(() => {
    const timer = window.setTimeout(() => onExpire(toast.id), TOAST_TTL_MS);
    return () => window.clearTimeout(timer);
  }, [toast.id, onExpire]);
  return (
    <div className={"toast" + (toast.tone === "error" ? " error" : "")}>
      <div className="toast-title">{toast.title}</div>
      <div className="toast-body">{toast.body}</div>
      {toast.action != null && (
        <button
          type="button"
          className="toast-action"
          onClick={toast.action.onClick}
        >
          {toast.action.label}
        </button>
      )}
    </div>
  );
}
