/// TransfersViewModel — 传输列表页视图模型(spec §4.5 传输列表页;镜像
/// android TasksReducer/TasksViewModel 的已裁定语义,宿主可测):
/// - **数据面**:transfer.list 全量重置(daemon 快照 = job_seq 升序 → 反转
///   即「最新在最上」)+ event.transfer 增量合并(原位更新;新行插顶;
///   **终态粘滞**——终态后迟到帧整体丢弃;**会话行终态即消失**——
///   incoming/local_copy 无持久记录面,不清即幽灵;事件帧不带 name/src/dst,
///   保留前值);
/// - **分段过滤四档**(全部/接收/发送/调度;「接收」含被动 incoming);
/// - **左滑动作集按状态**(spec §4.5 逐条,只列规格面动作,pause/resume
///   契约在而 UI 不上此页):进行中(queued/running/paused/publishing)→
///   [取消];中断/失败 → [重新发起, 清除];已取消/完成 → [清除]
///   (v2-P5 T5b Ruling K:done 恢复清除入口,「清除已完成」全语义);
///   会话行终态无动作(无记录面)。**同签名在途抑制**:中断/失败/已取消/
///   完成行若存在另一非终态同 kind+src+dst 行,终态动作一并隐藏(重新发起
///   会双发同 rels 坏对端 .part,清除会删对端续传位图;android 同口径);
/// - **完成时间**:契约不带时间戳(transfer.list/event.transfer 均无)——
///   只对**现场观测到 done 翻转**的行记录本地时间(绿勾+时间);种子已
///   完成的历史行不编造,时间缺省不显示(task-9 报告注记)。
///
/// 每个仓库调用均经 ``RpcGuard/withTimeout`` 限时(T3 缓办携带同款)。

import Foundation

/// 分段过滤四档(spec §4.5;顺序 = 全部/接收/发送/调度)。
public enum TransferFilter: String, CaseIterable, Identifiable, Sendable {
    case all
    case receive
    case send
    case schedule

    public var id: String { rawValue }

    public var label: String {
        switch self {
        case .all: "全部"
        case .receive: "接收"
        case .send: "发送"
        case .schedule: "调度"
        }
    }

    /// 段过滤(「接收」含被动来件 incoming,android FILTERS 同口径)。
    func matches(_ kind: String) -> Bool {
        switch self {
        case .all: true
        case .send: kind == "upload"
        case .receive: kind == "download" || kind == "incoming"
        case .schedule: kind == "remote_copy"
        }
    }
}

/// 行左滑动作(spec §4.5 三分集;UI 据此出键,VM 据此发帧)。
public enum TransferRowAction: Equatable, Sendable {
    case cancel
    case reinitiate
    case clear
}

/// 传输行呈现投影(纯函数;android TasksReducer 呈现面同象,字段口径与
/// daemon jobs.rs/api.rs 对齐)。
public enum TransfersFormat {

    /// 终态集(daemon is_terminal_state + store 独有 interrupted)。
    public static let terminalStates: Set<String> = ["done", "failed", "cancelled", "interrupted"]
    /// 可清除终态(v2-P5 T5b,Ruling K:引擎侧放开 done——原「done 拒清」
    /// 与 spec「清除已完成」冲突;四终态同权,残留清理随记录走)。
    public static let clearableStates: Set<String> = ["done", "failed", "cancelled", "interrupted"]
    /// 会话级 kind(daemon 无持久记录;终态动作必报「任务不存在」)。
    static let sessionKinds: Set<String> = ["incoming", "local_copy"]
    /// 有持久记录的 kind(终态动作资格面)。
    static let recordKinds: Set<String> = ["upload", "download", "remote_copy"]

    public static func isTerminal(_ state: String) -> Bool { terminalStates.contains(state) }
    /// 可清除(终态且有记录面;done 已放开,见 ``clearableStates`` 注)。
    public static func isClearable(_ state: String) -> Bool { clearableStates.contains(state) }
    static func isSessionKind(_ kind: String) -> Bool { sessionKinds.contains(kind) }
    static func isRecordKind(_ kind: String) -> Bool { recordKinds.contains(kind) }

    /// 类型徽章文案(接收蓝/发送绿/调度紫灰由 UI 按 label 上色;未知灰档防御)。
    public static func badgeLabel(for kind: String) -> String {
        switch kind {
        case "upload": "发送"
        case "download", "incoming": "接收"
        case "remote_copy": "调度"
        default: "其他"
        }
    }

    /// 任务名(单文件 = 名;多文件 = 首项名 + 「· N 项」;空名回退 id)。
    public static func displayName(_ job: TransferJob) -> String {
        let base = job.name.isEmpty ? job.id : job.name
        return job.filesTotal > 1 ? "\(base) · \(job.filesTotal) 项" : base
    }

    /// 状态文案(queued/running 归并「进行中」;未知状态原样,不崩)。
    public static func stateText(_ state: String, error: String?) -> String {
        switch state {
        case "publishing": "保存中"
        case "queued", "running": "进行中"
        case "paused": "已暂停"
        case "interrupted": "已中断"
        case "failed":
            (error?.hasPrefix("保存失败") ?? false) ? "保存失败" : "失败"
        case "done": "完成"
        case "cancelled": "已取消"
        default: state
        }
    }

    /// 中断/失败红因文案(spec §4.5「中断=红字原因」):引擎 error 在场即
    /// 原因(如设备离线),空则兜底「已中断」/「失败」;非终态 nil。
    public static func causeText(_ state: String, error: String?) -> String? {
        switch state {
        case "interrupted":
            let reason = error.flatMap { $0.isEmpty ? nil : $0 }
            return reason.map { "已中断 · \($0)" } ?? "已中断"
        case "failed":
            return error.flatMap { $0.isEmpty ? nil : $0 } ?? "失败"
        default:
            return nil
        }
    }

    /// 跳过段(R3 真机轮 #3/#6:跳过的重复机制可见化):skipped > 0 →
    /// 「跳过 N 项」;0/负值 → nil(不显示)。消费面 = TransferRowView
    /// done 态副文。(R7 评审 tail:去「同名已存在」括注——跳过成因不止
    /// 同名(照片截断账同落此面),语境中立两态皆准。)
    public static func skippedText(_ skipped: Int64) -> String? {
        guard skipped > 0 else { return nil }
        return "跳过 \(skipped) 项"
    }

    /// 进度比:总量未知(≤0,起步前/远端计数面)→ nil(不确定态进度条)。
    public static func progressRatio(bytesDone: Int64, bytesTotal: Int64) -> Double? {
        guard bytesTotal > 0 else { return nil }
        return min(max(Double(bytesDone) / Double(bytesTotal), 0), 1)
    }

    /// 速率文案(android speedText 同口径:0 → 「0 B/s」;<1024 整数;
    /// 此后 1024 进制一位小数)。
    public static func speedText(_ bps: Int64) -> String {
        if bps <= 0 { return "0 B/s" }
        if bps < 1024 { return "\(bps) B/s" }
        let units = ["KB", "MB", "GB", "TB"]
        var v = Double(bps) / 1024
        var i = 0
        while v >= 1024 && i < units.count - 1 {
            v /= 1024
            i += 1
        }
        return String(format: "%.1f %@/s", v, units[i])
    }

    /// 方向设备文案(spec §4.5「如公司台式机 → 本机」):src/dst 设备段解析
    /// (upload dst = `{to}:{dir}`、download src = `{dev}:{paths}`、
    /// remote_copy 两侧同形、incoming src = 裸设备 id;首冒号前即设备段,
    /// Windows 路径盘符冒号不受影响);目录名解析缺省回退裸 id,缺面「…」。
    public static func directionText(_ job: TransferJob, resolve: (String) -> String) -> String {
        func dev(_ side: String?) -> String? {
            guard let side, !side.isEmpty else { return nil }
            let head = side.split(separator: ":", maxSplits: 1, omittingEmptySubsequences: false).first
            let id = head.map(String.init) ?? side
            return id.isEmpty ? nil : resolve(id)
        }
        switch job.kind {
        case "upload":
            return "本机 → \(dev(job.dst) ?? "…")"
        case "download", "incoming":
            return "\(dev(job.src) ?? "…") → 本机"
        case "remote_copy":
            return "\(dev(job.src) ?? "…") → \(dev(job.dst) ?? "…")"
        default:
            return "…"
        }
    }

    /// 左滑动作集按状态(spec §4.5;pairs 见 TransfersViewModel 类注)。
    /// 终态动作集(v2-P5 T5b Ruling K,done 恢复清除入口):中断/失败 →
    /// [重新发起, 清除];已取消/完成 → [清除](引擎侧 done 已放开,
    /// spec「清除已完成」全语义;desktop done 同口径)。
    public static func actions(for job: TransferJob, rows: [TransferJob]) -> [TransferRowAction] {
        switch job.state {
        case "queued", "running", "paused", "publishing":
            return [.cancel]
        case "interrupted", "failed":
            return (isRecordKind(job.kind) && !hasInFlightSibling(job, rows: rows)) ? [.reinitiate, .clear] : []
        case "cancelled", "done":
            return (isRecordKind(job.kind) && !hasInFlightSibling(job, rows: rows)) ? [.clear] : []
        default:
            return []
        }
    }

    /// 同签名在途行存在性(签名 = kind+src+dst,同落位面;android 同口径)。
    static func hasInFlightSibling(_ job: TransferJob, rows: [TransferJob]) -> Bool {
        rows.contains {
            $0.id != job.id && !isTerminal($0.state)
                && $0.kind == job.kind && $0.src == job.src && $0.dst == job.dst
        }
    }
}

/// 传输列表页视图模型(标签根页单枚;StateObject 随页生命周期)。
@MainActor
public final class TransfersViewModel: ObservableObject {

    /// 单次调用限时(秒;进程内 UDS 本可短,统一取设备页现拉余量口径)。
    static let callTimeout: TimeInterval = 10

    // MARK: 只读状态(UI 订阅)

    /// 行集(**最新在最上**:daemon 快照反转 + 事件新行插顶)。
    @Published public private(set) var rows: [TransferJob] = []
    @Published public private(set) var loading = false
    @Published public private(set) var errorText: String?
    /// 行动作在途(防连点;左滑钮置灰)。
    @Published public private(set) var busy: Set<String> = []
    /// 最近一次动作失败文案(重新发起/清除/取消;UI 提示后置 nil 清除)。
    @Published public var lastActionError: String?

    // MARK: UI 输入

    /// 分段过滤(默认全部)。
    @Published public var filter: TransferFilter = .all
    /// 设备目录名(方向设备文案解析;视图随 event.devices 喂入,缺名回退裸 id)。
    @Published public var deviceNames: [String: String] = [:]

    /// 可见行 = 分段过滤(序不变,最新仍在前)。
    public var visibleRows: [TransferJob] {
        rows.filter { filter.matches($0.kind) }
    }

    // MARK: 依赖

    private let repo: MtdssRepository
    /// 现拉序号守卫(过期 transfer.list 应答不得覆盖新位)。
    private var loadSeq = 0
    /// 现场观测的完成时间(仅 done 翻转;种子历史行无,见类注)。
    private var completionTimes: [String: Date] = [:]

    public init(repo: MtdssRepository) {
        self.repo = repo
    }

    // MARK: 现拉/事件收敛

    /// transfer.list 全量重置(进入页/「···」刷新/动作后对账/前台回归同一入口)。
    public func load() async {
        loading = true
        errorText = nil
        loadSeq += 1
        let mySeq = loadSeq
        do {
            let result = try await RpcGuard.withTimeout(seconds: Self.callTimeout) { [repo] in
                try await repo.transferList()
            }
            guard mySeq == loadSeq else { return }
            rows = Self.project(result.jobs)
            completionTimes = completionTimes.filter { id, _ in rows.contains { $0.id == id } }
            loading = false
        } catch {
            guard mySeq == loadSeq else { return }
            // 行集保留旧值(android 同口径;错误横幅承担恢复入口)
            loading = false
            errorText = BrowseViewModel.userErrorText(for: error)
        }
    }

    /// event.transfer 增量合并(纯状态面):
    /// - 既有行:终态粘滞(迟到帧整体忽略);会话行终态 → 摘行;done 翻转
    ///   记完成时间;其余原位以事件帧为准(name/src/dst 保留前值);
    /// - 全新 id:会话 kind 的终态帧不建行,其余插顶(显示名兜底 = id,
    ///   name/src/dst 待对账补齐)。回传是否新行(视图可触发补拉)。
    @discardableResult
    public func apply(_ p: TransferProgress) -> Bool {
        guard let idx = rows.firstIndex(where: { $0.id == p.id }) else {
            guard !(TransfersFormat.isSessionKind(p.kind) && TransfersFormat.isTerminal(p.state)) else { return false }
            if p.state == "done" { completionTimes[p.id] = Date() }
            rows.insert(
                TransferJob(
                    id: p.id, kind: p.kind, name: p.id, state: p.state,
                    filesDone: p.filesDone, filesTotal: p.filesTotal,
                    bytesDone: p.bytesDone, bytesTotal: p.bytesTotal,
                    speedBps: p.speedBps, skipped: p.skipped, relay: p.relay,
                    src: nil, dst: nil, error: p.error,
                ),
                at: 0,
            )
            return true
        }
        let prev = rows[idx]
        if TransfersFormat.isTerminal(prev.state) { return false } // 终态粘滞
        if TransfersFormat.isSessionKind(p.kind) && TransfersFormat.isTerminal(p.state) {
            rows.remove(at: idx)
            return false
        }
        if p.state == "done" { completionTimes[p.id] = Date() }
        rows[idx] = TransferJob(
            id: p.id, kind: p.kind,
            name: prev.name.isEmpty ? p.id : prev.name,
            state: p.state,
            filesDone: p.filesDone, filesTotal: p.filesTotal,
            bytesDone: p.bytesDone, bytesTotal: p.bytesTotal,
            speedBps: p.speedBps, skipped: p.skipped, relay: p.relay,
            src: prev.src, dst: prev.dst, error: p.error,
        )
        return false
    }

    /// 行完成时间(现场观测;无 = 历史行,不显示)。
    public func completionTime(of id: String) -> Date? {
        completionTimes[id]
    }

    // MARK: 行动作(busy 防连点;reinitiate/clear 成功后重拉对账)

    public func cancel(_ id: String) async {
        await control(id, op: .cancel)
    }

    /// 重新发起(P2 语义:翻新同记录换新 job_id,有续传基础则续传;daemon
    /// 对 bridge 形态源「无可传内容」拒并保留原记录——错误面上浮)。
    public func reinitiate(_ id: String) async {
        await control(id, op: .reinitiate, reseedAfter: true)
    }

    /// 清除(记录摘除 + 残留回收;成功后重拉让行出列)。
    public func clear(_ id: String) async {
        await control(id, op: .clear, reseedAfter: true)
    }

    /// 「···」清除已完成(spec §4.5)。v2-P5 T5b 全语义(Ruling K):引擎
    /// 侧 done 已放开——本面清**全部有记录面的终态行**(完成/中断/失败/
    /// 已取消);同签名在途抑制同单行口径。批内逐条防连点,毕后一次对账。
    public func clearCompleted() async {
        let ids = rows
            .filter { TransfersFormat.isClearable($0.state) && TransfersFormat.isRecordKind($0.kind) }
            .filter { !TransfersFormat.hasInFlightSibling($0, rows: rows) }
            .map(\.id)
        for id in ids {
            await control(id, op: .clear)
        }
        if !ids.isEmpty {
            await load()
        }
    }

    /// 方向设备文案(VM 面便捷:目录名解析接 deviceNames)。
    public func directionText(for job: TransferJob) -> String {
        let names = deviceNames
        return TransfersFormat.directionText(job) { names[$0] ?? $0 }
    }

    // MARK: 内部

    /// 种子投影:终态会话行滤除 + id 去重防御 + **反转**(daemon 快照
    /// job_seq 升序 → 最新在最上)。
    static func project(_ jobs: [TransferJob]) -> [TransferJob] {
        var seen = Set<String>()
        return jobs
            .filter { !(TransfersFormat.isSessionKind($0.kind) && TransfersFormat.isTerminal($0.state)) }
            .filter { seen.insert($0.id).inserted }
            .reversed()
    }

    /// 行动作种别(control 的路由面;契约帧 = transfer.{cancel,reinitiate,clear} {id})。
    private enum ControlOp: Sendable {
        case cancel
        case reinitiate
        case clear
    }

    /// 动作统一执行面:busy 防连点 → 限时 RPC → 失败落一次性反馈;
    /// reinitiate/clear 成功后重拉对账(新行入场/旧行出列即刻反映)。
    private func control(_ id: String, op: ControlOp, reseedAfter: Bool = false) async {
        guard !busy.contains(id) else { return }
        busy.insert(id)
        defer { busy.remove(id) }
        do {
            try await RpcGuard.withTimeout(seconds: Self.callTimeout) { [repo] in
                switch op {
                case .cancel: try await repo.transferCancel(id: id)
                case .reinitiate: try await repo.transferReinitiate(id: id)
                case .clear: try await repo.transferClear(id: id)
                }
            }
            lastActionError = nil
            if reseedAfter {
                await load()
            }
        } catch {
            lastActionError = BrowseViewModel.userErrorText(for: error)
        }
    }
}
