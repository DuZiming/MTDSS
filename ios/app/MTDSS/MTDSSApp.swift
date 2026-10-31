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

/// 应用级模型(T4 最小面):引擎 boot 就绪即持有 Repository,失败挂横幅。
/// 浏览/设备/传输各屏(AppTab 四标签)由 T5/T7/T9/T14+ 逐层实装消费。
@MainActor
@Observable
final class AppModel {
    private(set) var repository: MtdssRepository?
    private(set) var bootError: String?

    /// 引擎进进程(spec §5.1):App 启动即在进程内线程跑引擎主循环,就绪以
    /// hello 通为准(EngineHost 轮询)。幂等:已就绪不重复 boot。
    func bootEngine() async {
        guard repository == nil else { return }
        do {
            repository = try await ProductionEngine.bootRepository()
        } catch {
            bootError = "引擎启动失败:\(error)"
        }
    }
}
