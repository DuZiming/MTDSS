import SwiftUI
import MTDSSKit
import UIKit

@main
struct MTDSSApp: App {
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

    /// 引擎进进程(spec §5.1):App 启动即在进程内线程跑引擎主循环,就绪以
    /// hello 通为准(EngineHost 轮询)。幂等:已就绪不重复 boot。
    func bootEngine() async {
        guard repository == nil else { return }
        do {
            let repo = try await ProductionEngine.bootRepository()
            repository = repo
            eventHub = EventHub(client: repo.ipc)
            await refreshStatus()
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
        }
        let events = hub.subscribe()
        for await event in events {
            if case .transfer(let p) = event {
                activityTracker.apply(p)
                transferActive = activityTracker.isActive
            }
        }
    }
}
