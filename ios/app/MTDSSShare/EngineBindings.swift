/// 扩展侧引擎装配(R10,brief 架构 ①:扩展链接引擎;T4 同款分层——绑定归
/// 扩展目标,逻辑归 Kit):`@_silgen_name` 直绑 libios_bridge.a 三符号后
/// 构造 Kit ``EngineHost/Bindings`` 注入。静态库与主 App 同一份 .a(device
/// slice;扩展只进真机形态,sim slice 不需要——构建仍走 build.sh,工程面
/// project.yml 为 MTDSSShare 增 .a 依赖)。
///
/// 引擎 home = `{App Group}/mtdss`(``EngineSessionSync/extensionHomeName``;
/// 简报原案 `engine-home` 命名在真机 App Group 容器路径下 daemon.sock 达
/// 107 字节,超 UDS sun_path 104 上限——预算钉测 SessionSyncTests 双面钉,
/// 此处运行时再用 ``EngineLayout/socketPathsFitSunPath`` 复验,超限 = 直发
/// 整体禁用回落队列)。素材由主 App 会话同步(Kit ``EngineSessionSync``)
/// 预置 → 扩展引擎 boot 即同设备身份、已登录。
///
/// 红线(T2 minor #3 携带):不触发 set_recv_dir,不写 settings.json。

import Foundation
import MTDSSKit
import os

/// 扩展引擎日志(R10 排障面;Console 按 subsystem top.mtdss.ios.share /
/// category engine 过滤)。
let shareEngineLog = Logger(subsystem: "top.mtdss.ios.share", category: "engine")

// MARK: C ABI 直绑(签名逐参对照 crates/ios-bridge/src/lib.rs;与主 App
// EngineHost.swift 同型——扩展目标独立进程,绑定各链一份)

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

// MARK: 生产装配 + home 派生

enum ExtensionEngine {

    /// 真 C ABI 注入的宿主(start/ipc_path/stop 三件套;Kit EngineHost
    /// 消费,hello 背避就绪轮询同主 App)。
    static let host = EngineHost(bindings: EngineHost.Bindings(
        start: { mtdss_ios_start($0) },
        stop: { mtdss_ios_stop() },
        ipcPath: { mtdss_ios_ipc_path($0, $1, $2) },
    ))

    /// 扩展引擎 home(App Group 容器下 `mtdss/`;容器不可得 = nil)。
    static func home(fileManager: FileManager = .default) -> String? {
        guard let groupRoot = fileManager.containerURL(
            forSecurityApplicationGroupIdentifier: AppGroup.identifier)
        else { return nil }
        return EngineSessionSync.extensionHomeURL(groupRoot: groupRoot).path
    }

    /// 直发环境自检:home 可得 + 会话素材在场(主 App 已同步)+ UDS 预算
    /// 内。任一不满足 = 直发禁用(调用方降级队列;日志留因)。
    static func directSendAvailable(home: String?) -> Bool {
        guard let home else {
            shareEngineLog.info("direct-send unavailable: no app-group container")
            return false
        }
        guard FileManager.default.fileExists(atPath: home + "/" + EngineSessionSync.sessionFile),
              FileManager.default.fileExists(atPath: home + "/" + EngineSessionSync.identityFile)
        else {
            shareEngineLog.info("direct-send unavailable: session material missing (open app to sync)")
            return false
        }
        let layout = EngineLayout(home: home)
        guard layout.socketPathsFitSunPath else {
            shareEngineLog.error("direct-send unavailable: socket path over sun_path budget \(layout.daemonSocketPath, privacy: .public)")
            return false
        }
        return true
    }
}
