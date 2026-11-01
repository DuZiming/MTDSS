/// 可取消的一次性回调等待(T11 评审 I-1/I-2):
/// 形态:发起一次系统异步操作(如 NSExtensionContext.open 的
/// completionHandler)并等它回话——但系统侧可能**永不回调**(share 扩展
/// 拉起宿主在部分系统形态静默无回),竞窗超时必须能真正收口。
///
/// - ``withCallbackFallback``:**取消感知续体**。任务被取消(竞窗败方/
///   调用方取消)→ 立即以 `false` 收口;回调与取消竞先 → **恰一次续**,
///   迟到方吞(不重续不崩);回调先到 → 原值。取消先于挂起(已取消任务
///   进入)→ 回退值即时收口,attempt 照常发起(迟到回调被吞)。
/// - ``firstOrTimeout``:竞窗(run 与定时器取先回者;超时回 nil)。退出前
///   cancelAll——败方子任务须经取消感知面收口,否则任务组隐式等全部子
///   任务、竞窗形同虚设(评审 I-1 的挂死形态;ShareQueueTests 钉死)。
///
/// App 侧用法(分享扩展 openHost):
/// ```
/// withCallbackFallback { onDone in
///     await MainActor.run { ctx.open(url) { onDone($0) } }  // open 须主线程(I-2)
/// }
/// ```

import Foundation

/// 恰一次续体持有(回调/取消竞先收口;锁内 settled 翻转防重续)。
final class CallbackOnce<T: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<T, Never>?
    private var settled = false
    private let fallback: T

    init(fallback: T) {
        self.fallback = fallback
    }

    /// 回调路径(系统 completionHandler 与取消处理器共用)。
    func settle(_ value: T) {
        lock.lock()
        if settled {
            lock.unlock()
            return
        }
        settled = true
        let cont = continuation
        continuation = nil
        lock.unlock()
        cont?.resume(returning: value)
    }

    /// 续体挂接(取消已先到 → 立即按 fallback 收口)。
    func wait() async -> T {
        await withCheckedContinuation { cont in
            lock.lock()
            if settled {
                lock.unlock()
                cont.resume(returning: fallback)
                return
            }
            continuation = cont
            lock.unlock()
        }
    }
}

/// 等系统回调或取消;取消(或已取消)→ `false` 收口。
public func withCallbackFallback(
    _ attempt: @escaping @Sendable (_ onDone: @escaping @Sendable (Bool) -> Void) async -> Void,
) async -> Bool {
    let once = CallbackOnce<Bool>(fallback: false)
    return await withTaskCancellationHandler(operation: {
        await attempt { once.settle($0) }
        return await once.wait()
    }, onCancel: {
        once.settle(false)
    })
}

/// 竞窗:run 与定时器取先回者;超时回 nil。败方经 cancelAll 取消收口
/// (调用方传入的 run 若不响应取消,本 helper 会挂——run 须取消感知,
/// 如 ``withCallbackFallback``)。
public func firstOrTimeout<T: Sendable>(
    seconds: TimeInterval,
    _ run: @escaping @Sendable () async -> T,
) async -> T? {
    await withTaskGroup(of: T?.self) { group in
        group.addTask { await run() }
        group.addTask {
            try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
            return nil
        }
        let first = await group.next() ?? nil
        group.cancelAll()
        return first
    }
}
