import SwiftUI
import MTDSSKit

@main
struct MTDSSApp: App {
    @State private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            AppTabView()
                .environment(model)
                .task { await model.bootEngine() }
        }
    }
}

/// 应用级模型(T4 最小面 + T5 status 投影):引擎 boot 就绪即持有
/// Repository;常用目录根数据源(recv_dir/self device id)经 ``refreshStatus``
/// 投影。浏览/设备/传输各屏(AppTab 四标签)由 T5/T7/T9/T14+ 逐层实装消费。
@MainActor
@Observable
final class AppModel {
    private(set) var repository: MtdssRepository?
    private(set) var bootError: String?

    /// 本机 device_id(status.device;未登录为 null——登录面归 T14)。
    private(set) var selfDeviceId: String?
    /// 收件目录(status.recv_dir;常用目录根「收件箱」条目的路径)。
    private(set) var recvDir: String?

    /// 引擎进进程(spec §5.1):App 启动即在进程内线程跑引擎主循环,就绪以
    /// hello 通为准(EngineHost 轮询)。幂等:已就绪不重复 boot。
    func bootEngine() async {
        guard repository == nil else { return }
        do {
            repository = try await ProductionEngine.bootRepository()
            await refreshStatus()
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
}
