import SwiftUI
import MTDSSKit
import UIKit

@main
struct MTDSSApp: App {
    @Environment(\.scenePhase) private var scenePhase
    @State private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            AppTabView()
                .environment(model)
                .task { await model.bootEngine() }
                // 传输进行中亮屏,终态复位(spec §5.5;Android KEEP_SCREEN_ON 同象)
                .onChange(of: model.transferActive) { _, active in
                    UIApplication.shared.isIdleTimerDisabled = active
                }
                // 前台回归重对齐(T6-minor3 携带):种子→订阅竞窗内错过的
                // 事件以 transfer.list 快照收口(亮屏面校正;通知去重集对账)
                .onChange(of: scenePhase) { _, phase in
                    if phase == .active {
                        Task { await model.realignOnActive() }
                    }
                }
        }
    }
}

/// 应用级模型(T4 最小面 + T5 status 投影 + T6 传输活动面):引擎 boot
/// 就绪即持有 Repository;常用目录根数据源(recv_dir/self device id)经
/// ``refreshStatus`` 投影;传输活动 ``transferActive`` 事件驱动(spec §5.5
/// 亮屏面,AppTabView 据此翻转 isIdleTimerDisabled)。浏览/设备/传输各屏
/// (AppTab 四标签)由 T5/T6/T7/T9/T14+ 逐层实装消费。
@MainActor
@Observable
final class AppModel {
    private(set) var repository: MtdssRepository?
    private(set) var bootError: String?

    /// 本机 device_id(status.device;未登录为 null——登录面归 T14)。
    private(set) var selfDeviceId: String?
    /// 收件目录(status.recv_dir;常用目录根「收件箱」条目的路径)。
    private(set) var recvDir: String?

    /// 任一非终态任务在传(spec §5.5:isIdleTimerDisabled 的应置值)。
    /// 事件驱动 + boot 快照种子;Android KEEP_SCREEN_ON 同口径。
    private(set) var transferActive = false
    private let activityTracker = TransferActivityTracker()
    /// 事件扇出(T6 消费传输活动;T9 传输列表另开订阅,同源不抢流)。
    private var eventHub: EventHub?
    /// 传输完成本地通知(spec §3;T9):判定面 Kit 可测
    /// (``TransferCompletionAnnouncer``),UN 绑定在 App 目标
    /// (Support/LocalNotify.swift;首次通知时请求一次授权)。
    private let notifier: any LocalNotifier
    private var completionAnnouncer = TransferCompletionAnnouncer()

    init(notifier: any LocalNotifier = UNLocalNotifier()) {
        self.notifier = notifier
    }

    /// 引擎进进程(spec §5.1):App 启动即在进程内线程跑引擎主循环,就绪以
    /// hello 通为准(EngineHost 轮询)。幂等:已就绪不重复 boot。
    func bootEngine() async {
        guard repository == nil else { return }
        do {
            let repo = try await ProductionEngine.bootRepository()
            repository = repo
            eventHub = EventHub(client: repo.ipc)
            await refreshStatus()
            // 收件发布器(T8,spec §3/§5.2):fs.publish → move 暂存件进
            // Documents;fs.query_dest → 收件箱真实行。独立订阅流(传输活动
            // 监视/设备页各占一条,同源不抢流)。
            if let hub = eventHub {
                let events = hub.subscribe()
                let layout = ProductionEngine.layout()
                Task { await InboxPublish.run(layout: layout, events: events) }
            }
            Task { await runTransferMonitor() }
        } catch {
            bootError = "引擎启动失败:\(error)"
        }
    }

    /// status 投影(常用目录根数据源;进入标签时幂等重拉,限时防挂)。
    /// 失败静默保留旧值(引擎横幅另有 bootError 兜底)。
    func refreshStatus() async {
        guard let repo = repository else { return }
        do {
            let status = try await RpcGuard.withTimeout(seconds: 5) { [repo] in
                try await repo.status()
            }
            selfDeviceId = status.device?.deviceId
            recvDir = status.recvDir
        } catch {
            // 保留旧值:常用目录以既有 recv_dir 呈现(首轮为空则空态)
        }
    }

    /// 事件订阅句柄(设备页 event.devices 收敛 T7;传输列表 T9 同源另开,
    /// 不抢流)。引擎未 boot 为 nil。
    func subscribeEvents() -> AsyncStream<IpcEvent>? {
        eventHub?.subscribe()
    }

    /// 传输活动消费环(app 生命周期常驻):boot 快照种子(重启恢复的非终态
    /// 任务同样占亮屏)→ event.transfer 逐帧喂追踪器;终态复位即熄屏。
    private func runTransferMonitor() async {
        guard let repo = repository, let hub = eventHub else { return }
        if let snapshot = try? await RpcGuard.withTimeout(seconds: 10) { [repo] in
            try await repo.transferList()
        } {
            activityTracker.seed(snapshot.jobs)
            transferActive = activityTracker.isActive
            // boot 快照对账:快照里已 done 的作业记入通知去重集——引擎对
            // 存量记录 boot 即发初始帧,历史完成不得再触发通知(只通知
            // 本次会话内新完成的;前台回归 realignOnActive 同口径)
            completionAnnouncer.reconcile(snapshot.jobs)
        }
        let events = hub.subscribe()
        for await event in events {
            if case .transfer(let p) = event {
                activityTracker.apply(p)
                transferActive = activityTracker.isActive
                // 完成通知(spec §3):首条 done 帧即发(会话内每作业恰一次)
                if let done = completionAnnouncer.announce(p) {
                    await announceCompletion(done)
                }
            }
        }
    }

    /// 前台回归重对齐(T6-minor3 携带):boot 种子→订阅建立之间存在竞窗,
    /// 窗内翻转的事件会被错过——前台 transfer.list 快照全量重置追踪器校正
    /// transferActive;完成通知去重集对账(快照已 done 的作业不再由迟到
    /// 通道补发通知,历史完成不打扰)。
    func realignOnActive() async {
        guard let repo = repository else { return }
        if let snapshot = try? await RpcGuard.withTimeout(seconds: 10) { [repo] in
            try await repo.transferList()
        } {
            activityTracker.seed(snapshot.jobs)
            transferActive = activityTracker.isActive
            completionAnnouncer.reconcile(snapshot.jobs)
        }
    }

    /// 完成通知文案:任务名经 transfer.list 现拉(事件帧不带 name;会话级
    /// 来件终态后仍在注册表快照内,可考);取不到回退「传输任务」。多文件
    /// 注记总项数(与列表行「· N 项」同语言)。
    private func announceCompletion(_ p: TransferProgress) async {
        var name = ""
        if let repo = repository,
           let list = try? await RpcGuard.withTimeout(seconds: 10) { [repo] in
            try await repo.transferList()
        } {
            name = list.jobs.first { $0.id == p.id }?.name ?? ""
        }
        let base = name.isEmpty ? "传输任务" : name
        let body = p.filesTotal > 1 ? "\(base) · 共 \(p.filesTotal) 项" : base
        await notifier.notify(title: "传输完成", body: body)
    }
}
