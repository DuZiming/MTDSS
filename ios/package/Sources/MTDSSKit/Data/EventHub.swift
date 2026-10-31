/// EventHub — 事件扇出:AsyncStream 单消费 → 多订阅者(UI 多屏各取一份;
/// 镜像 Kotlin SharedFlow 的多方订阅语义)。订阅流带缓冲(bufferingNewest,
/// 慢订阅丢旧帧);close 后全部订阅流终结。

import Foundation

public final class EventHub: @unchecked Sendable {
    private let lock = NSLock()
    private var subscribers: [UUID: AsyncStream<IpcEvent>.Continuation] = [:]
    /// 声明处默认值先满足 definite-initialization,init 内重赋真实消费任务
    /// (Task 闭包捕获 self 须在全部存储属性就绪之后)。
    private var task = Task<Void, Never> {}

    public init(_ source: AsyncStream<IpcEvent>) {
        task = Task { [weak self] in
            for await event in source {
                guard let self else { return }
                self.broadcast(event)
            }
            self?.finishAll()
        }
    }

    public convenience init(client: IpcClient) {
        self.init(client.events)
    }

    deinit {
        task.cancel()
        finishAll()
    }

    /// 新开一条订阅流(订阅即刻生效;弃用/终止由 onTermination 自动摘除)。
    public func subscribe(buffering: Int = 64) -> AsyncStream<IpcEvent> {
        let (stream, continuation) = AsyncStream.makeStream(
            of: IpcEvent.self,
            bufferingPolicy: .bufferingNewest(buffering),
        )
        let id = UUID()
        lock.lock()
        subscribers[id] = continuation
        lock.unlock()
        continuation.onTermination = { [weak self] _ in
            self?.remove(id)
        }
        return stream
    }

    /// 停止扇出并终结全部订阅流。幂等。
    public func close() {
        task.cancel()
        finishAll()
    }

    // MARK: 内部

    private func broadcast(_ event: IpcEvent) {
        lock.lock()
        let continuations = Array(subscribers.values)
        lock.unlock()
        for c in continuations {
            c.yield(event)
        }
    }

    private func finishAll() {
        lock.lock()
        let continuations = subscribers.values
        subscribers.removeAll()
        lock.unlock()
        for c in continuations {
            c.finish()
        }
    }

    private func remove(_ id: UUID) {
        lock.lock()
        subscribers[id] = nil
        lock.unlock()
    }
}
