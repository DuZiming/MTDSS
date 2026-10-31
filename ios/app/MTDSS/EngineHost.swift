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

    /// MTDSS 数据目录:App Documents(spec §5.2 收件箱位;桥自动注入
    /// authorized_roots = {home}/inbox)。
    ///
    /// DEBUG 档可用环境变量 MTDSS_HOME 覆盖 —— 模拟器容器路径远超 UDS 的
    /// sun_path 104 上限(macOS 宿主路径 ≈170+ 字节,bind 必败),模拟器
    /// 冒烟经 `SIMCTL_CHILD_MTDSS_HOME=短路径 xcrun simctl launch` 指定短
    /// 目录;真机容器路径(≈100 字节)贴边可容,不设 env 即走 Documents。
    static func home() -> String {
        #if DEBUG
        if let custom = ProcessInfo.processInfo.environment["MTDSS_HOME"], !custom.isEmpty {
            return custom
        }
        #endif
        return FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].path
    }
}
