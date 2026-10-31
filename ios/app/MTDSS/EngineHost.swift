/// App 侧引擎装配(T2 C ABI ↔ MTDSSKit.EngineHost 的接合层):
/// - `@_silgen_name` 直绑 libios_bridge.a 三符号(纯 C 面,免桥接头);
///   静态库由 ios/build.sh 拷入 Engine/ 并随本目标链接。
/// - Kit 层 EngineHost 以注入闭包消费(Ruling E:宿主逻辑归 Kit 可测,绑定
///   归 App;mock 测试见 MTDSSKitTests/EngineHostTests)。
///
/// 红线(T2 minor #3 携带):iOS UI 不得触发 set_recv_dir —— guard 配置下
/// 会把收件箱挪出授权集(引擎自断浏览);本文件亦不写 settings.json。

import Foundation
import MTDSSKit

// MARK: C ABI 直绑(签名逐参对照 crates/ios-bridge/src/lib.rs)

@_silgen_name("mtdss_ios_start")
private func mtdss_ios_start(_ home_path: UnsafePointer<CChar>?) -> Int32

@_silgen_name("mtdss_ios_stop")
private func mtdss_ios_stop() -> Int32

@_silgen_name("mtdss_ios_ipc_path")
private func mtdss_ios_ipc_path(
    _ home_path: UnsafePointer<CChar>?,
    _ out: UnsafeMutablePointer<CChar>?,
    _ out_len: Int,
) -> Int32

// MARK: 生产装配

enum ProductionEngine {
    /// 真 C ABI 注入的宿主(start/ipc_path/stop 三件套)。
    static let host = EngineHost(bindings: EngineHost.Bindings(
        start: { mtdss_ios_start($0) },
        stop: { mtdss_ios_stop() },
        ipcPath: { mtdss_ios_ipc_path($0, $1, $2) },
    ))

    /// 引擎进进程 + hello 就绪,就绪即回 Repository(App 各屏数据门面)。
    static func bootRepository() async throws -> MtdssRepository {
        let client = try await host.start(home: home())
        return MtdssRepository(client: client)
    }

    /// 布局派生单一家(App 与桥共享同一 home 入参;桥内派生 Documents/
    /// staging/授权根,Kit 侧 `EngineLayout` 同款规则供发布器定位 fds.sock
    /// 与收件箱)。
    static func layout() -> EngineLayout {
        EngineLayout(home: home())
    }

    /// MTDSS 数据目录(T8,spec §5.2):真机 = App 私有
    /// `{container}/Library/mtdss`(Files App 不可见;收件箱/发布根 = 容器
    /// Documents 由桥按 home 父目录派生,Files App「MTDSS」可见)。
    ///
    /// DEBUG 档可用环境变量 MTDSS_HOME 覆盖 home 整体 —— 模拟器容器路径
    /// 远超 UDS 的 sun_path 104 上限(macOS 宿主路径 ≈170+ 字节,bind 必败),
    /// 模拟器冒烟经 `SIMCTL_CHILD_MTDSS_HOME=短目录 xcrun simctl launch`
    /// 指定短目录(其下 `../Documents` 即发布根,与真机布局同规则);真机
    /// 容器路径最坏 102 字节(<104)可容,不设 env 即走 Library/mtdss。
    static func home() -> String {
        #if DEBUG
        if let custom = ProcessInfo.processInfo.environment["MTDSS_HOME"], !custom.isEmpty {
            return custom
        }
        #endif
        let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].path
        return EngineLayout(containerDocuments: documents).home
    }
}
