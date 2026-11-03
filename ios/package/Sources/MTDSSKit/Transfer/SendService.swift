/// SendService — 发送/下载/转发/本地删除的请求组装面(T6/T7):产品语义在
/// 此收口(spec §3),调用方只给「选了什么、发给谁」,契约帧形态由本面铸造
/// (SendServiceTests 与 crates/ipc/contract golden 逐字段锁定):
/// - **send**(发送免选目录):本机 → 对端收件箱。transfer.start 路径形态
///   upload(源 = 授权根内绝对路径,引擎源守卫收口),dir = 收件箱哨兵
///   "inbox"——**全平台通发**(R12:Ruling I 的 v1 拒发退役;T5b **Ruling
///   J** 引擎侧哨兵统一已落地,unix 收端 `dir="inbox"` 按非法回退默认收件
///   目录,与 Windows 同语义,零协议变更;v1 期间「unix 桌面对端拒发」的
///   旧立场随用户 20261103.3 设备轮「列表不一致」报告一并退役——App 设备
///   列表全平台可选,扩展面板同口径)。
/// - **download**:远端 → 本机收件箱。dir = 本机 recv_dir 绝对路径
///   (desktop-cfg 引擎口径:iOS 不用哨兵,哨兵仅 android cfg 放行);
///   recv_dir 经 status 现拉(App 未登录/引擎未就绪时失败透传)。
/// - **forward**(转发 = 调度):远端 A → 远端 B 的 remote_copy,数据不经
///   操作端;dir = 收件箱哨兵(B 端落点,全平台同口径)。
/// - **deleteLocal**:本机选中项删除(ctrl delete,device = 本机 id;
///   force = true——iOS 引擎无回收设施,恒永久删,UI 侧先行确认)。
///
/// 每次调用经 ``RpcGuard/withTimeout`` 限时(T3 缓办携带,同 BrowseViewModel)。
/// 宿主可测:纯 MTDSSKit,无 SwiftUI。

import Foundation

public struct SendService: Sendable {

    /// 收件箱哨兵(与 daemon intake::INBOX_SENTINEL 同值;android MtdssRepository
    /// INBOX_DIR 同源)。
    public static let inboxSentinel = "inbox"

    /// 默认单次调用限时(秒;本地引擎调用:download 的 status 现拉、本地删除)。
    public static let callTimeout: TimeInterval = 30

    /// TRANSFER-START 受理调用专项限时(秒;send/forward/download 三个
    /// transferStart* 调用点):daemon 受理要现场与对端建 ctrl 通道,冷对端
    /// 首跳握手 T7 实测 25–35s,30s 默认会在「对端真收下作业」时先到——
    /// UI 报失败而 daemon 已受理、文件照达(R5c E2E 首跑实证假失败窗)。
    /// 90s 只为盖住握手窗:最坏 35s 握手 + start 本身余量充足;其余调用
    /// 保持默认预算,单发不重试语义不变。
    public static let transferStartTimeout: TimeInterval = 90

    private let repo: MtdssRepository

    public init(repo: MtdssRepository) {
        self.repo = repo
    }

    // MARK: 发送(本机 → 对端收件箱)

    /// 发送选中项到对端收件箱(全平台哨兵口径,Ruling J 引擎侧统一;返回
    /// 新作业 id)。
    public func send(paths: [String], to peer: DirectoryDevice) async throws -> String {
        let result = try await Self.withTimeout(seconds: Self.transferStartTimeout) {
            try await repo.transferStartUpload(
                files: paths, to: peer.deviceId, dir: Self.inboxSentinel,
            )
        }
        return result.jobId
    }

    // MARK: 下载(远端 → 本机收件箱)

    /// 下载远端条目到本机收件箱(recv_dir 现拉);返回新作业 id。
    public func download(device: String, paths: [String]) async throws -> String {
        let recvDir = try await Self.withTimeout {
            try await self.repo.status().recvDir
        }
        guard let recvDir, !recvDir.isEmpty else {
            throw RpcError(code: 1, message: "收件目录未知(引擎未就绪或未登录)")
        }
        let result = try await Self.withTimeout(seconds: Self.transferStartTimeout) {
            try await self.repo.transferStartDownload(
                device: device, paths: paths, dir: recvDir,
            )
        }
        return result.jobId
    }

    // MARK: 转发(远端 A → 远端 B 调度)

    /// 转发:A(来源设备)选中项 → B 收件箱(remote_copy 作业,B 端 dir =
    /// 收件箱哨兵,全平台同口径);返回新作业 id。
    public func forward(from: String, paths: [String], to peer: DirectoryDevice) async throws -> String {
        let result = try await Self.withTimeout(seconds: Self.transferStartTimeout) {
            try await self.repo.transferStartRemoteCopy(
                from: from, paths: paths, to: peer.deviceId, dir: Self.inboxSentinel,
            )
        }
        return result.jobId
    }

    // MARK: 本地删除

    /// 删除本机选中项(永久删;iOS 引擎恒 force 语义,recycled 恒 false)。
    public func deleteLocal(device: String, paths: [String]) async throws -> DeleteResult {
        try await Self.withTimeout {
            try await self.repo.delete(device: device, paths: paths, force: true)
        }
    }

    // MARK: 内部

    /// 统一限时闸(SendServiceTests 的调用链都在此超时口径内);缺省 =
    /// 默认预算,transfer-start 受理调用点显式传 ``transferStartTimeout``
    /// (status 现拉/本地删除走缺省不动)。
    private static func withTimeout<T: Sendable>(
        seconds: TimeInterval = callTimeout,
        _ run: @escaping @Sendable () async throws -> T,
    ) async throws -> T {
        try await RpcGuard.withTimeout(seconds: seconds, run)
    }
}

/// 传输活动追踪(spec §5.5:传输进行中亮屏 `isIdleTimerDisabled=true`,
/// 终态复位)。事件驱动:event.transfer 逐帧喂 ``apply``;boot 时以
/// transfer.list 快照 ``seed``(重启恢复的非终态任务同样占亮屏)。
/// 终态集 = done/failed/cancelled/interrupted(android TasksReducer 同口径;
/// paused/发布窗 publishing 属非终态——接收/保存中同样不应熄屏)。
@MainActor
public final class TransferActivityTracker: ObservableObject {

    /// 非终态(亮屏占用)作业 id 集。
    @Published public private(set) var activeIds: Set<String> = []

    /// 是否存在任一非终态任务(= isIdleTimerDisabled 的应置值)。
    public var isActive: Bool { !activeIds.isEmpty }

    /// 终态集(android TasksReducer TERMINAL_STATES 同源)。
    public static let terminalStates: Set<String> = ["done", "failed", "cancelled", "interrupted"]

    public init() {}

    /// event.transfer 增量:终态帧摘除,非终态帧占位。
    public func apply(_ p: TransferProgress) {
        if Self.terminalStates.contains(p.state) {
            activeIds.remove(p.id)
        } else {
            activeIds.insert(p.id)
        }
    }

    /// transfer.list 快照全量重置(boot/重连对账)。
    public func seed(_ jobs: [TransferJob]) {
        activeIds = Set(jobs.filter { !Self.terminalStates.contains($0.state) }.map(\.id))
    }
}
