/// RpcGuard — 限时调用闸(EngineHost.withTimeout 的公开面)。T3 缓办
/// 携带(终审 triage):IpcClient 挂起调用无内建超时,无应答时永久 pending
/// ——UI 侧每个 Repository 调用都必须限时赛跑,超时即失败返回,不让等待
/// 占住界面(EngineHost 就绪轮询同款 task 组赛跑手法)。

import Foundation

public enum RpcGuard {

    /// 赛跑败者错误(超时;调用方映射用户文案)。
    public struct TimeoutError: Error, Equatable, Sendable, CustomStringConvertible {
        public init() {}
        public var description: String { "调用超时" }
    }

    /// 带总时长的 await:run 与计时器 task 组赛跑,先到先得,败者取消。
    public static func withTimeout<T: Sendable>(
        seconds: TimeInterval,
        _ run: @escaping @Sendable () async throws -> T,
    ) async throws -> T {
        try await withThrowingTaskGroup(of: T.self) { group in
            group.addTask { try await run() }
            group.addTask {
                try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
                throw TimeoutError()
            }
            let first = try await group.next()!
            group.cancelAll()
            return first
        }
    }
}
