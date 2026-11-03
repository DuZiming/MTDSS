/// 直发编排(R10,brief 架构 ③;用户裁定:点设备 → 表内进度条 → 完成,
/// 无排队无拉起无「好」钮):会话素材检查 → 批次帽 → 两步暂存 → conf 授权
/// → 引擎 boot(Kit ``EngineHost``,真 C ABI 经 ``ExtensionEngine``)→
/// ``SendService/send``(Ruling I 哨兵口径)→ transfer 事件泵逐帧喂
/// ``ShareSendFlow`` → 终态 → 停引擎 → 收口。
///
/// **降级阶梯**(全部回落既有队列,主 App 下次打开消费——两级拉起代码
/// 退役,open()/responder-chain 真机实证不拉起,R6/R9 记录在案):
/// - 无 App Group 容器 → failed(容器不可用,队列也不可达);
/// - 会话素材缺(主 App 未同步/未登录)→ 队列(旧 enqueue 拷贝路);
/// - 超批次帽(>50 件或 >200MB)→ 队列(同上,暂存都不做);
/// - 引擎 boot 失败 → 已暂存批 **rename 收编**入队列(``ShareQueueStore/
///   adoptFiles``,同卷零拷贝)→ 队列;
/// - 发送受理失败(对端冷握手超时等)与传输终态 failed/interrupted →
///   failed(重试[重选目标,引擎仍热]/转队列双钮)。
///
/// 状态面 = ``ShareSendState``(@Observable 盒;视图 @State 持有,编排经
/// 引用直改,SwiftUI 观察随 property 翻转——结构体状态机的观察面,免闭包
/// 穿梭)。编排自身 @MainActor。os_log 全程(subsystem top.mtdss.ios.share)。

import Foundation
import MTDSSKit
import Observation
import os

/// 直发流状态盒(@Observable;视图 @State 持有)。
@MainActor
@Observable
final class ShareSendState {
    var flow = ShareSendFlow()
}

@MainActor
final class DirectSendEngine {

    /// 直发运行结果(视图按此收口:done 1s 后 complete / cancelled 短停 /
    /// queued 注记 / failed 等用户动作)。
    enum Outcome: Equatable {
        case sent
        case cancelled
        case failed
        case queued
    }

    private let home: String?
    private let layout: EngineLayout?
    private let staging: SendStaging?
    private let queue: ShareQueueStore?
    private let host: EngineHost
    /// 引擎热客户端(boot 一次复用:failed 重试重选不重 boot)。
    private var client: IpcClient?
    private var repo: MtdssRepository?
    private var hub: EventHub?
    /// 当前暂存批(failed 相位保留供重试/收编;sent/cancelled 清弃;
    /// queued 收编后置 nil)。
    private var batch: URL?
    private var batchFiles: [ShareQueueFile] = []
    private var batchPaths: [String] = []
    /// 在途作业 id(取消钮消费;终态即清)。
    private var activeJobId: String?
    /// 引擎授权根已就位(conf 已写 + 已 boot)。
    private var rootsReady = false

    init(
        home: String?, staging: SendStaging?, queue: ShareQueueStore?,
        host: EngineHost = ExtensionEngine.host,
    ) {
        self.home = home
        self.layout = home.map { EngineLayout(home: $0) }
        self.staging = staging
        self.queue = queue
        self.host = host
    }

    /// 生产装配(App Group 三面 + 扩展宿主;容器不可得 = 直发禁用)。
    static func make() -> DirectSendEngine {
        DirectSendEngine(
            home: ExtensionEngine.home(),
            staging: SendStaging.appGroupRoot(),
            queue: ShareQueueStore.appGroupRoot().map { ShareQueueStore(root: $0) })
    }

    deinit {
        // 进程被系统回收而无收口路径:停引擎线程 + 清残余批(MainActor 隔离
        // 的 shutdown() 不可自 deinit 调,最小面重复;落盘状态本就为准)。
        if client != nil { host.stop() }
        if let batch { staging?.removeBatch(batch) }
    }

    /// 直发可行性(面板装载时判一次;不可行 = 点选直接走队列降级)。
    var available: Bool {
        ExtensionEngine.directSendAvailable(home: home)
    }

    // MARK: 面板生命周期

    /// 装载时孤儿批清扫(上次扩展会话残批 = 死流,>1h 清弃)。
    func purgeOrphanBatches() {
        guard let staging else { return }
        staging.purgeOrphans(keeping: nil, olderThan: 3600, now: Date())
    }

    /// 收口(视图 completeRequest 前):停引擎 + 清弃残余批(queued 路径
    /// 批已收编/置 nil)。completeRequest 后扩展进程随时可被回收,落盘为准。
    func shutdown() {
        if client != nil {
            shareEngineLog.info("engine stop begin")
            host.stop()
            client = nil
            repo = nil
            hub = nil
            rootsReady = false
            shareEngineLog.info("engine stop done")
        }
        if let batch {
            staging?.removeBatch(batch)
            self.batch = nil
        }
    }

    // MARK: 一次点选 = 一轮直发

    func run(
        device: SharedDeviceEntry, payloads: [SharePayload], state: ShareSendState,
    ) async -> Outcome {
        let deviceId = device.deviceId
        let totalBytes = payloads.reduce(Int64(0)) { $0 + $1.size }

        // ① 可行性/帽:不可行或越帽 → 不暂存,旧队列拷贝路直接降级
        guard available else {
            return enqueueFromPayloads(deviceId: deviceId, payloads: payloads, state: state)
        }
        guard ShareDirectSendLimits.eligible(count: payloads.count, totalBytes: totalBytes) else {
            shareEngineLog.info("over cap items=\(payloads.count) bytes=\(totalBytes) → queue fallback")
            return enqueueFromPayloads(deviceId: deviceId, payloads: payloads, state: state)
        }

        // ② 暂存批(failed 重试路径复用既有批,不重拷)
        do {
            if batch == nil {
                try stageBatch(payloads: payloads)
            }
        } catch {
            let message = "暂存失败:\(error.localizedDescription)"
            shareEngineLog.error("stage FAIL \(message, privacy: .public)")
            state.flow.fallbackFailed(deviceId: deviceId, message: message)
            return .failed
        }

        // ③ 授权根 + 引擎 boot(幂等:热引擎复用);失败 → 批收编入队列
        do {
            try await ensureBooted()
        } catch {
            shareEngineLog.error("boot FAIL: \(String(describing: error), privacy: .public) → queue fallback")
            return adoptIntoQueue(deviceId: deviceId, state: state)
        }

        // ④ 事件先订后发(评审 R10-1:hub 不重放——受理应答与订阅之间
        // 落地的 transfer 帧[瞬时小传输/事件先到形]曾会丢,流恒卡 sending;
        // 先订后发 + 泵内 jobId 过滤 = 零丢帧窗口)
        guard let hub else {
            state.flow.fallbackFailed(deviceId: deviceId, message: "引擎事件面未就绪")
            return .failed
        }
        let events = hub.subscribe()
        guard let repo else {
            return adoptIntoQueue(deviceId: deviceId, state: state)
        }
        let peer = DirectoryDevice(
            deviceId: device.deviceId, name: device.name,
            platform: device.platform, online: device.online)
        let jobId: String
        do {
            jobId = try await SendService(repo: repo).send(paths: batchPaths, to: peer)
        } catch {
            let message = ShareQueueConsumer.errorText(of: error)
            shareEngineLog.error("send accept FAIL \(message, privacy: .public)")
            state.flow.fallbackFailed(deviceId: deviceId, message: message)
            return .failed
        }
        activeJobId = jobId
        state.flow.sendAccepted(
            jobId: jobId, names: payloads.map(\.name), sizes: payloads.map(\.size))
        shareEngineLog.info("job accepted id=\(jobId) files=\(payloads.count)")

        // ⑤ 事件泵:transfer 逐帧喂流,终态收口
        return await pump(events: events, jobId: jobId, deviceId: deviceId, state: state)
    }

    /// 取消(发送中取消钮):transfer.cancel → 引擎终态 cancelled 帧收口。
    func cancel() async {
        guard let repo, let jobId = activeJobId else { return }
        shareEngineLog.info("cancel requested job=\(jobId)")
        try? await repo.transferCancel(id: jobId)
    }

    /// failed 相位的「转队列」用户动作:有暂存批 = 收编(零拷贝);无批
    /// (暂存前失败形)= 旧拷贝路。返回 .queued/.failed(流已随翻转)。
    func queueFallback(
        deviceId: String, payloads: [SharePayload], state: ShareSendState,
    ) -> Outcome {
        if batch != nil {
            return adoptIntoQueue(deviceId: deviceId, state: state)
        }
        return enqueueFromPayloads(deviceId: deviceId, payloads: payloads, state: state)
    }

    // MARK: 阶梯内 step

    /// 事件泵:job 匹配的 transfer 帧喂 apply,终态翻转即回。流由 run()
    /// 在受理调用**之前**订阅(R10-1:hub 不重放,晚订丢终态帧)。引擎侧
    /// 作业状态机对失败/中断恒发终态帧(R5-R9 契约面)——终态前的无终态
    /// 挂起仅存于引擎僵死角(进程内引擎无此先例),不设泵内超时;事件流
    /// 终结(hub 收口/引擎退出)而无终态帧 → failed。
    private func pump(
        events: AsyncStream<IpcEvent>, jobId: String, deviceId: String, state: ShareSendState,
    ) async -> Outcome {
        for await event in events {
            guard case .transfer(let p) = event, p.id == jobId else { continue }
            state.flow.apply(
                filesDone: p.filesDone, bytesDone: p.bytesDone, bytesTotal: p.bytesTotal)
            guard TransferActivityTracker.terminalStates.contains(p.state) else { continue }
            guard state.flow.terminal(state: p.state, jobId: jobId, error: p.error) else {
                continue
            }
            activeJobId = nil
            shareEngineLog.info("job terminal state=\(p.state, privacy: .public)")
            switch p.state {
            case "done":
                return .sent
            case "cancelled":
                return .cancelled
            default:
                return .failed
            }
        }
        if activeJobId == jobId {
            activeJobId = nil
            state.flow.fallbackFailed(deviceId: deviceId, message: "引擎连接中断")
        }
        return .failed
    }

    /// 暂存批两步拷入(payload 临时区 → App Group 批;尺寸守卫)。
    private func stageBatch(payloads: [SharePayload]) throws {
        guard let staging else {
            throw ShareEnqueueError.queueUnavailable
        }
        let id = UUID().uuidString
        let dir = try staging.newBatch(id: id)
        var files: [ShareQueueFile] = []
        for payload in payloads {
            let file = ShareQueueFile(
                name: ShareQueueStore.sanitizedName(payload.name), size: payload.size)
            try staging.stageFile(batch: dir, file: file, from: payload.tmpURL)
            files.append(file)
        }
        batch = dir
        batchFiles = files
        batchPaths = files.map { staging.filePath(in: dir, name: $0.name) }
        shareEngineLog.info("batch staged id=\(id) files=\(files.count)")
    }

    /// 授权根(conf 写暂存根)+ 引擎 boot(一次;热引擎直接复用)。
    private func ensureBooted() async throws {
        if rootsReady, client != nil { return }
        guard let home, let layout, let staging else {
            throw RpcError(code: 1, message: "共享容器不可用")
        }
        guard layout.socketPathsFitSunPath else {
            throw RpcError(code: 1, message: "引擎路径超 UDS 预算")
        }
        try FileManager.default.createDirectory(at: staging.root, withIntermediateDirectories: true)
        // conf = 暂存根(桥 start 读入与容器 Documents 并集;Documents 在
        // 扩展形态不存在,fail-closed 滤除——授权集即暂存根)。start 读
        // conf,故恒先写后 boot。
        try EngineRootsConf.write(home: home, paths: [staging.root.path])
        shareEngineLog.info("boot begin home=\(home, privacy: .public)")
        let client = try await host.start(home: home)
        repo = MtdssRepository(client: client)
        hub = EventHub(client: client)
        self.client = client
        rootsReady = true
        shareEngineLog.info("boot ok")
    }

    /// 降级:已暂存批 rename 收编入队列(同卷零拷贝;批与队列同容器)。
    private func adoptIntoQueue(deviceId: String, state: ShareSendState) -> Outcome {
        guard let queue, let staging, let batch else {
            state.flow.fallbackFailed(deviceId: deviceId, message: "共享容器不可用")
            return .failed
        }
        do {
            let id = UUID().uuidString
            try queue.adoptFiles(
                item: id, targetDeviceId: deviceId, files: batchFiles,
                fromDirectory: batch, now: Date())
            self.batch = nil
            batchFiles = []
            batchPaths = []
            state.flow.fallbackQueued(deviceId: deviceId)
            shareEngineLog.info("batch adopted into queue item=\(id)")
            return .queued
        } catch {
            let message = ShareQueueConsumer.errorText(of: error)
            state.flow.fallbackFailed(deviceId: deviceId, message: message)
            return .failed
        }
    }

    /// 降级:未暂存面(不可行/超帽)直接按旧路两步拷入队列(主 App 消费;
    /// Kit ``ShareQueueStore/enqueueFiles`` 单一实现,半途失败不落坏项)。
    private func enqueueFromPayloads(
        deviceId: String, payloads: [SharePayload], state: ShareSendState,
    ) -> Outcome {
        guard let queue, !payloads.isEmpty else {
            state.flow.fallbackFailed(deviceId: deviceId, message: "共享容器不可用")
            return .failed
        }
        do {
            try queue.enqueueFiles(
                id: UUID().uuidString, targetDeviceId: deviceId,
                files: payloads.map {
                    ShareQueueFile(name: ShareQueueStore.sanitizedName($0.name), size: $0.size)
                },
                sources: payloads.map(\.tmpURL), now: Date())
            state.flow.fallbackQueued(deviceId: deviceId)
            shareEngineLog.info("payloads enqueued (fallback) items=\(payloads.count)")
            return .queued
        } catch {
            let message = ShareQueueConsumer.errorText(of: error)
            state.flow.fallbackFailed(deviceId: deviceId, message: message)
            return .failed
        }
    }
}
