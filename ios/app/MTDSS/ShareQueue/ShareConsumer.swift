/// 分享队列生产装配(T11,Ruling E:逻辑归 Kit 可测,绑定归 App):
/// - ``ShareCenter``(@MainActor):消费触发(boot/event.login/前台)→
///   ``ShareQueueConsumer``(Kit)执行「校验→暂存收件箱→发送→清队列」;
///   失败行投影给传输列表(「分享转交失败」,可重试/清除);
/// - 设备缓存写入(设备事件 + 前台 + boot):devices 快照 → App Group,
///   扩展读之渲染目标设备行(>5min 过期置灰在扩展侧判);
/// - 发送闭包:SendService.send 的 Ruling I 口径(目标设备从目录现拉,
///   unix 桌面对端拒发文案同源)。
///
/// App Group 是扩展↔主 App 的唯一共享面(spec §5.4);引擎 home 不参与
/// (扩展无引擎访问权)。红线(T2 携带):本文件不触发 set_recv_dir。

import Foundation
import MTDSSKit
import os

/// 消费环日志(R6 #4c:冷启动 openurl 拉起 → 消费链每步可查;Console 按
/// subsystem top.mtdss.ios / category share-consume 过滤)。
private let shareConsumeLog = Logger(subsystem: "top.mtdss.ios", category: "share-consume")

@MainActor
@Observable
final class ShareCenter {

    /// 传输列表失败行投影(「分享转交失败」)。
    struct FailureRow: Identifiable, Equatable {
        let id: String
        let name: String
        let filesTotal: Int
        let totalBytes: Int64
        let reason: String?
    }

    /// 失败行(传输列表顶部呈现;scan 驱动)。
    private(set) var failedRows: [FailureRow] = []
    /// 消费进行中(防 foreground/login 双触发并发扫同一队列)。
    private var consuming = false

    /// 队列面(App Group 不可得 = 功能整体禁用,行/消费全空)。
    let store: ShareQueueStore?
    /// 设备缓存写入面(同容器;nil 同上)。
    let deviceCache: SharedDeviceCache?

    /// 收件箱绝对路径(暂存目标;引擎布局派生,boot 后注入)。
    private var inboxPath: String?

    init(store: ShareQueueStore?, deviceCache: SharedDeviceCache?) {
        self.store = store
        self.deviceCache = deviceCache
    }

    /// 生产装配(App Group 容器;布局收件箱)。
    static func make() -> ShareCenter {
        let root = ShareQueueStore.appGroupRoot()
        return ShareCenter(
            store: root.map { ShareQueueStore(root: $0) },
            deviceCache: root.map { SharedDeviceCache(root: $0) })
    }

    // MARK: 消费(boot / event.login(true) / 前台回归)

    /// 一轮消费:登录门(未登录不动队列——保「打开 App 即自动发送」的
    /// 等待态,而非把整队折失败)→ 消费 → 失败行刷新。
    func consumeIfNeeded(repository: MtdssRepository?, loggedIn: Bool?) async {
        guard let store, !consuming else { return }
        guard loggedIn == true, let repository else { return }
        let inbox = inboxPath ?? ProductionEngine.layout().inbox
        inboxPath = inbox
        consuming = true
        defer { consuming = false }
        let consumer = ShareQueueConsumer(
            queue: store, inboxPath: inbox, send: Self.sender(repository: repository))
        shareConsumeLog.info("consume begin (trigger=boot/foreground/login)")
        let outcome = await consumer.consumeOnce()
        shareConsumeLog.info("consume done consumed=\(outcome.consumed)")
        refreshRows()
    }

    /// 重试单条失败行(传输列表重试 → 重选目标 → 本入口;覆盖目标设备)。
    func retry(id: String, targetDeviceId: String, repository: MtdssRepository) async {
        guard let store else { return }
        let inbox = inboxPath ?? ProductionEngine.layout().inbox
        inboxPath = inbox
        let consumer = ShareQueueConsumer(
            queue: store, inboxPath: inbox, send: Self.sender(repository: repository))
        await consumer.retryOnce(id: id, overrideTargetDeviceId: targetDeviceId)
        refreshRows()
    }

    /// 清除失败行(队列项移除;暂存文件留在收件箱——用户文件不触碰)。
    func clear(id: String) {
        guard let store else { return }
        try? store.removeItem(id: id)
        refreshRows()
    }

    /// 失败行刷新(scan → failed 投影)。
    func refreshRows() {
        guard let store else {
            failedRows = []
            return
        }
        let entries = (try? store.scan(now: Date())) ?? []
        failedRows = entries
            .filter { $0.meta.status == .failed }
            .map { entry in
                FailureRow(
                    id: entry.id,
                    name: entry.meta.sourceName.isEmpty ? "分享内容" : entry.meta.sourceName,
                    filesTotal: max(entry.meta.files.count, 1),
                    totalBytes: entry.meta.size,
                    reason: entry.meta.error)
            }
    }

    // MARK: 设备缓存写入(扩展的设备行数据源)

    /// devices 快照写入 App Group(boot/前台/设备事件;失败静默——扩展侧
    /// 有陈旧置灰兜底,缓存缺半拍无碍)。
    func writeDeviceCache(devices: [DirectoryDevice]) {
        guard let deviceCache else { return }
        try? deviceCache.write(
            devices: devices.map(SharedDeviceEntry.init), now: Date())
    }

    // MARK: 发送闭包(Ruling I 收口)

    /// 生产发送:(收件箱内路径组, 目标设备 id) → 作业 id。目标设备面
    /// 现拉(SendService 需平台判 Ruling I;设备被移除即明确失败文案)。
    static func sender(
        repository: MtdssRepository,
    ) -> @Sendable ([String], String) async throws -> String {
        let sendService = SendService(repo: repository)
        return { paths, deviceId in
            let directory = try await RpcGuard.withTimeout(seconds: SendService.callTimeout) {
                try await repository.devices()
            }
            guard let peer = directory.devices.first(where: { $0.deviceId == deviceId }) else {
                throw RpcError(code: 1, message: "目标设备已不在设备列表")
            }
            return try await sendService.send(paths: paths, to: peer)
        }
    }
}
