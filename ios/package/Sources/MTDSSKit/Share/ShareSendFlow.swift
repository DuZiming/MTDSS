/// 直发流状态机(R10;用户裁定 20261103.2:选设备 → 表内进度条 → 完成,
/// 无排队、无拉起主 App、无「好」钮;分享扩展链引擎直发):
///
///     idle ──pick(d)──▶ busy(d)(拷暂存+boot 引擎)──sendAccepted──▶
///     sending(d, job)(进度推进)──terminal──▶ done(d) / cancelled(d) /
///                                            failed(d, msg)(重试/转队列)
///     busy ──fallbackQueued──▶ queued(d)(降级注记,主 App 下次打开消费)
///     busy ──fallbackFailed──▶ failed(d, msg)
///     failed ──pick(d2)──▶ busy(重试 = 重选目标,引擎仍热,重发同一暂存批)
///
/// - **降级触发面**(brief R10:boot/登录失败、无会话同步、超批次帽 → 队列;
///   受理失败[对端冷握手超时等]→ failed 双钮)。两级拉起代码退役(open()/
///   responder-chain 真机实证不拉起,R6/R9 记录在案)。
/// - **逐文件进度**:引擎 transfer 事件只有作业级 files_done/bytes_done;
///   本机按「文件按序处理」推导:前 filesDone 件全完成、第 filesDone+1 件
///   分摊 bytesDone − 已完成件字节和(钳制)、其余待发。
/// - 纯状态机,宿主可测(Ruling E);编排(boot/send/事件泵)在扩展目标
///   ``DirectSendEngine``。
public struct ShareSendFlow: Equatable, Sendable {

    /// 流相位。
    public enum Phase: Equatable, Sendable {
        case idle
        /// 暂存拷贝 + 引擎 boot 进行中(行内 spinner,整表禁点)。
        case busy(deviceId: String)
        /// 作业已受理,进度推进中(取消钮在场)。
        case sending(deviceId: String, jobId: String)
        /// 传输完成(视图短暂停留后收口扩展)。
        case done(deviceId: String)
        /// 用户取消(视图短暂停留后收口;暂存批清弃)。
        case cancelled(deviceId: String)
        /// 失败(原因 + 重试[重选目标]/转队列双钮)。
        case failed(deviceId: String, message: String)
        /// 降级队列(注记即刻呈现,视图短暂停留后收口)。
        case queued(deviceId: String)
    }

    public private(set) var phase: Phase = .idle

    /// 进度快照(sending 相位在场;nil = 未受理)。
    public private(set) var progress: DirectSendProgress?

    /// 受理时登记的清单(逐文件推导的输入)。
    private var manifest: [ShareQueueFile] = []

    public init() {}

    /// 点选设备(idle/failed → busy;queued = 队列已接管不重选;busy/
    /// sending/done/cancelled 中拒点)。返回 false = 拒点。
    @discardableResult
    public mutating func pick(deviceId: String) -> Bool {
        switch phase {
        case .idle, .failed:
            phase = .busy(deviceId: deviceId)
            progress = nil
            manifest = []
            return true
        case .busy, .sending, .done, .cancelled, .queued:
            return false
        }
    }

    /// 作业受理(busy → sending;进度种子 = 全零待发)。
    public mutating func sendAccepted(jobId: String, names: [String], sizes: [Int64]) {
        guard case .busy = phase else { return }
        manifest = zip(names, sizes).map { ShareQueueFile(name: $0, size: $1) }
        progress = DirectSendProgress(files: manifest.map {
            .init(name: $0.name, size: $0.size, bytesDone: 0)
        }, filesDone: 0, bytesDone: 0, bytesTotal: manifest.reduce(0) { $0 + $1.size })
        if case .busy(let deviceId) = phase {
            phase = .sending(deviceId: deviceId, jobId: jobId)
        }
    }

    /// 进度推进(sending 相位 only;他相位的迟到帧吞掉)。
    public mutating func apply(filesDone: Int64, bytesDone: Int64, bytesTotal: Int64) {
        guard case .sending = phase, !manifest.isEmpty else { return }
        progress = DirectSendProgress.derive(
            manifest: manifest, filesDone: filesDone, bytesDone: bytesDone, bytesTotal: bytesTotal)
    }

    /// 作业终态(sending 且 jobId 匹配才翻转;返回是否翻转)。
    /// done → done;cancelled → cancelled;其余(failed/interrupted/…)→
    /// failed(error 缺席兜底「传输失败(state)」)。
    @discardableResult
    public mutating func terminal(state: String, jobId: String, error: String?) -> Bool {
        guard case .sending(let deviceId, let activeJob) = phase, activeJob == jobId else {
            return false
        }
        switch state {
        case "done":
            phase = .done(deviceId: deviceId)
        case "cancelled":
            phase = .cancelled(deviceId: deviceId)
        default:
            phase = .failed(deviceId: deviceId, message: error ?? "传输失败(\(state))")
        }
        return true
    }

    /// 降级队列(busy[编排面自动降级]或 failed[用户「转队列」钮]→ queued;
    /// deviceId 须与在途相位一致。sending 中不降级——作业已受理,终态面接管)。
    public mutating func fallbackQueued(deviceId: String) {
        switch phase {
        case .busy(let id), .failed(let id, _):
            guard id == deviceId else { return }
            phase = .queued(deviceId: deviceId)
        case .idle, .sending, .done, .cancelled, .queued:
            break
        }
    }

    /// 降级收编失败(队列写入抛错形;busy → failed,可重试)。
    public mutating func fallbackFailed(deviceId: String, message: String) {
        guard case .busy(let busyId) = phase, busyId == deviceId else { return }
        phase = .failed(deviceId: deviceId, message: message)
    }
}

/// 直发批次帽(brief R10:件数 ≤ 50 且总量 ≤ 200MB 直发,越帽回落队列)。
public enum ShareDirectSendLimits: Sendable {
    public static let maxFiles = 50
    public static let maxTotalBytes: Int64 = 200 * 1024 * 1024

    /// 直发资格(非空单 + 双帽内)。
    public static func eligible(count: Int, totalBytes: Int64) -> Bool {
        count >= 1 && count <= maxFiles && totalBytes <= maxTotalBytes
    }
}

/// 直发进度快照(作业级事件帧 → 逐文件推导,见 ``ShareSendFlow`` 注)。
public struct DirectSendProgress: Equatable, Sendable {
    public struct FileState: Equatable, Sendable {
        public let name: String
        public let size: Int64
        /// 本文件已传字节(完成件 = size;在途件分摊;待发 = 0)。
        public let bytesDone: Int64
        public var isDone: Bool { bytesDone >= size }
        public var isSending: Bool { !isDone && bytesDone > 0 }

        public init(name: String, size: Int64, bytesDone: Int64) {
            self.name = name
            self.size = size
            self.bytesDone = bytesDone
        }
    }

    public let files: [FileState]
    public let filesDone: Int
    public let bytesDone: Int64
    public let bytesTotal: Int64

    /// 作业文件总数(引擎 files_total 的投影)。
    public var filesTotal: Int { files.count }

    /// 总百分比(0…100;总量 0 防除零)。
    public var overallPercent: Int {
        bytesTotal > 0 ? min(100, max(0, Int(bytesDone * 100 / bytesTotal))) : 0
    }

    public init(
        files: [FileState], filesDone: Int, bytesDone: Int64, bytesTotal: Int64,
    ) {
        self.files = files
        self.filesDone = filesDone
        self.bytesDone = bytesDone
        self.bytesTotal = bytesTotal
    }

    static func derive(
        manifest: [ShareQueueFile], filesDone: Int64, bytesDone: Int64, bytesTotal: Int64,
    ) -> DirectSendProgress {
        let doneCount = max(0, min(Int(filesDone), manifest.count))
        let completedBytes = manifest.prefix(doneCount).reduce(Int64(0)) { $0 + $1.size }
        // 在途余量:作业字节减已完成件字节;超前于 files_done 的事件帧
        // (字节先到、件数后到)按序溢入后续文件,不出负不超尺寸。
        var remainder = max(0, bytesDone - completedBytes)
        var states: [FileState] = []
        for (index, file) in manifest.enumerated() {
            let fileDone: Int64
            if index < doneCount {
                fileDone = file.size
            } else if remainder > 0 {
                fileDone = min(remainder, file.size)
                remainder -= fileDone
            } else {
                fileDone = 0
            }
            states.append(.init(name: file.name, size: file.size, bytesDone: fileDone))
        }
        return DirectSendProgress(
            files: states, filesDone: doneCount,
            bytesDone: max(0, bytesDone), bytesTotal: bytesTotal)
    }
}
