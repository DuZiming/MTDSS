/// SendService — 发送/下载/转发/本地删除的请求组装面(T6/T7):产品语义在
/// 此收口(spec §3),调用方只给「选了什么、发给谁」,契约帧形态由本面铸造
/// (SendServiceTests 与 crates/ipc/contract golden 逐字段锁定):
/// - **send**(发送免选目录):本机 → 对端收件箱。transfer.start 路径形态
///   upload(源 = 授权根内绝对路径,引擎源守卫收口),dir 按 **Ruling I**
///   随对端平台解析(见 ``PeerInboxTarget``):移动对端 = 收件箱哨兵
///   "inbox"(对端 App 应答发布查询,私有暂存+发布);Windows 桌面对端 =
///   同发哨兵(对端引擎「非法 warn+回退默认收件目录」既有口径,v2-P5 T6
///   E2E 实证);**unix 桌面对端(linux/macOS)v1 拒发**(裁定立场,终审
///   M-c 复核定稿)——T5b 携带的 NEEDS_CONTEXT 已由 **Ruling J** 收口:
///   引擎侧哨兵统一落地(unix 收端 `dir="inbox"` 按非法回退默认收件目录,
///   与 Windows 同语义,零协议变更,不再有「私有暂存+发布查询超时」挂
///   面);但 v1 期间 iOS 对 unix 桌面对端仍维持拒发(对端落点是其默认
///   收件目录而非收件箱语义,宁拒勿误投),v1 后随引擎侧统一重评。
/// - **download**:远端 → 本机收件箱。dir = 本机 recv_dir 绝对路径
///   (desktop-cfg 引擎口径:iOS 不用哨兵,哨兵仅 android cfg 放行);
///   recv_dir 经 status 现拉(App 未登录/引擎未就绪时失败透传)。
/// - **forward**(转发 = 调度):远端 A → 远端 B 的 remote_copy,数据不经
///   操作端;dir 按 **Ruling I** 随 B 平台解析(同 send 口径)。
/// - **deleteLocal**:本机选中项删除(ctrl delete,device = 本机 id;
///   force = true——iOS 引擎无回收设施,恒永久删,UI 侧先行确认)。
///
/// 每次调用经 ``RpcGuard/withTimeout`` 限时(T3 缓办携带,同 BrowseViewModel)。
/// 宿主可测:纯 MTDSSKit,无 SwiftUI。

import Foundation

/// 对端收件箱指向解析(Ruling I:契约无对端 recv_dir,dir 按对端平台收口)。
public enum PeerInboxTarget: Equatable, Sendable {
    /// 收件箱哨兵 "inbox" 可上线:移动对端(App 应答发布查询,哨兵语义完整)
    /// 或 Windows 桌面对端(对端回退默认收件目录,既有引擎口径)。
    case sentinel
    /// 哨兵不可上线:unix 桌面对端(T5b/Ruling J 引擎侧统一后收端回退
    /// 默认收件目录,不挂)与未知平台——v1 拒发并出文案(裁定立场,见
    /// ``send`` 文档),v1 后重评。
    case unavailable
}

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

    /// unix 桌面/未知对端拒发文案(Ruling I v1 缺口;daemon 消息风格)。
    public static let peerUnsupportedText =
        "该设备系统暂不支持免选目录接收(Linux/macOS 桌面对端将随后续版本开放)"

    private let repo: MtdssRepository

    public init(repo: MtdssRepository) {
        self.repo = repo
    }

    // MARK: Ruling I(dir 解析)

    /// 对端收件箱指向(platform = 服务器目录原样字符串,std::env::consts::OS
    /// 语言:windows/linux/macos/android/ios;nil/未知 = 无法判定)。
    public static func peerInboxTarget(platform: String?) -> PeerInboxTarget {
        switch platform?.lowercased() {
        case "android", "ios", "windows":
            .sentinel
        default:
            .unavailable
        }
    }

    // MARK: 发送(本机 → 对端收件箱)

    /// 发送选中项到对端收件箱(Ruling I dir 解析);返回新作业 id。
    public func send(paths: [String], to peer: DirectoryDevice) async throws -> String {
        guard Self.peerInboxTarget(platform: peer.platform) == .sentinel else {
            throw RpcError(code: 1, message: Self.peerUnsupportedText)
        }
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

    /// 转发:A(来源设备)选中项 → B 收件箱(remote_copy 作业,B 端 dir 按
    /// Ruling I 随其平台解析);返回新作业 id。
    public func forward(from: String, paths: [String], to peer: DirectoryDevice) async throws -> String {
        guard Self.peerInboxTarget(platform: peer.platform) == .sentinel else {
            throw RpcError(code: 1, message: Self.peerUnsupportedText)
        }
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
