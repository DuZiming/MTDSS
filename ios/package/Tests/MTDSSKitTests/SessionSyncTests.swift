/// ``EngineSessionSync`` 会话同步钉测(R10):主 App 引擎 home → 扩展引擎
/// home(App Group)的最小登录素材镜像。契约事实(crates/daemon):
/// - 登录态 = `session.json`(server/jwt/refresh/device_id/ca)+ 本机身份
///   `identity.seed`(flow.rs:identity 与 session 的 device_id 必须一致,
///   分歧即硬错)——**两者都不可少**;
/// - refresh 轮换是无状态 JWT 重铸(server auth.rs,无吊销注册表)→ 两份
///   副本各自自愈,无分歧风险;
/// - `ca` 字段是**绝对路径**(自签场景)→ 同步时必须拷贝证书并改写指向,
///   否则扩展 home 的引擎读不到(两沙箱不互通);
/// - `peers.json`(TOFU 缓存)缺省可再生 → 尽力拷贝,缺席不算失败。

import XCTest
@testable import MTDSSKit

final class SessionSyncTests: XCTestCase {

    private var appHome: URL!
    private var extHome: URL!

    override func setUpWithError() throws {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("sessionsync-\(UUID().uuidString)", isDirectory: true)
        appHome = base.appendingPathComponent("app", isDirectory: true)
        extHome = base.appendingPathComponent("ext", isDirectory: true)
        try FileManager.default.createDirectory(at: appHome, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: extHome, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(
            at: appHome.deletingLastPathComponent())
    }

    private func write(_ url: URL, _ text: String) throws {
        try Data(text.utf8).write(to: url)
    }

    private func sessionJSON(ca: String? = nil) -> String {
        let caPart = ca.map { ", \"ca\": \"\($0)\"" } ?? ""
        return """
        {"server": "https://www.mtdss.top", "jwt": "eyJ.a.b", "refresh": "r-1", \
        "device_id": "44d97f96"\(caPart)}
        """
    }

    // MARK: 基本镜像

    func testSyncCopiesSessionAndIdentity() throws {
        try write(appHome.appendingPathComponent("session.json"), sessionJSON())
        try write(appHome.appendingPathComponent("identity.seed"), "seed-bytes")
        XCTAssertTrue(EngineSessionSync.sync(appHome: appHome.path, extHome: extHome.path))
        XCTAssertEqual(
            try Data(contentsOf: extHome.appendingPathComponent("session.json")),
            try Data(contentsOf: appHome.appendingPathComponent("session.json")))
        XCTAssertEqual(
            try Data(contentsOf: extHome.appendingPathComponent("identity.seed")),
            try Data(contentsOf: appHome.appendingPathComponent("identity.seed")))
        // Unix 私有面:session/identity 落盘 0600(与 daemon write_private 同口径)
        for name in ["session.json", "identity.seed"] {
            let attrs = try FileManager.default.attributesOfItem(
                atPath: extHome.appendingPathComponent(name).path)
            #if canImport(Darwin)
            let mode = (attrs[.posixPermissions] as? NSNumber)?.int16Value ?? 0
            XCTAssertEqual(mode, 0o600, "\(name) 必须 0600")
            #endif
        }
    }

    /// 重同步(令牌轮换/重新登录)→ 内容更新为新值。
    func testResyncUpdatesStaleCopy() throws {
        try write(appHome.appendingPathComponent("session.json"), sessionJSON())
        try write(appHome.appendingPathComponent("identity.seed"), "seed-bytes")
        XCTAssertTrue(EngineSessionSync.sync(appHome: appHome.path, extHome: extHome.path))
        try write(appHome.appendingPathComponent("session.json"), sessionJSON().replacingOccurrences(of: "r-1", with: "r-2"))
        XCTAssertTrue(EngineSessionSync.sync(appHome: appHome.path, extHome: extHome.path))
        let copied = try String(contentsOf: extHome.appendingPathComponent("session.json"), encoding: .utf8)
        XCTAssertTrue(copied.contains("r-2"), "重同步必须更新到最新会话")
    }

    // MARK: 原子性(临时名+rename,不留 .tmp 半成品)

    func testSyncLeavesNoTempFiles() throws {
        try write(appHome.appendingPathComponent("session.json"), sessionJSON())
        try write(appHome.appendingPathComponent("identity.seed"), "seed-bytes")
        XCTAssertTrue(EngineSessionSync.sync(appHome: appHome.path, extHome: extHome.path))
        let leftovers = try FileManager.default.contentsOfDirectory(atPath: extHome.path)
            .filter { $0.hasSuffix(".tmp") || $0.hasSuffix(".part") }
        XCTAssertTrue(leftovers.isEmpty, "同步不得留临时残件:\(leftovers)")
    }

    // MARK: 未登录/素材不全 = 撤镜像(扩展侧据此降级队列)

    func testSyncWithoutSessionRemovesExtensionMaterial() throws {
        try write(appHome.appendingPathComponent("session.json"), sessionJSON())
        try write(appHome.appendingPathComponent("identity.seed"), "seed-bytes")
        XCTAssertTrue(EngineSessionSync.sync(appHome: appHome.path, extHome: extHome.path))
        // 登出:app home session 删除 → 重同步撤掉扩展侧全部登录素材
        try FileManager.default.removeItem(at: appHome.appendingPathComponent("session.json"))
        XCTAssertFalse(EngineSessionSync.sync(appHome: appHome.path, extHome: extHome.path))
        XCTAssertFalse(
            FileManager.default.fileExists(atPath: extHome.appendingPathComponent("session.json").path),
            "登出后扩展侧不得残留会话")
        XCTAssertFalse(
            FileManager.default.fileExists(atPath: extHome.appendingPathComponent("identity.seed").path))
    }

    /// identity.seed 缺失(素材不全):不得只同步半份——既不落 session,
    /// 也撤掉旧镜像(防「旧 session + 引擎新生成 seed」的 device_id 分歧)。
    func testSyncWithoutIdentityFailsAndClearsStaleMaterial() throws {
        try write(appHome.appendingPathComponent("session.json"), sessionJSON())
        // 素材不全(identity 缺)→ 同步失败
        XCTAssertFalse(EngineSessionSync.sync(appHome: appHome.path, extHome: extHome.path))
        XCTAssertFalse(
            FileManager.default.fileExists(atPath: extHome.appendingPathComponent("session.json").path))
        // 且后续同形态重入稳定返回 false(幂等)
        XCTAssertFalse(EngineSessionSync.sync(appHome: appHome.path, extHome: extHome.path))
    }

    // MARK: ca 证书拷贝与路径改写

    func testSyncCopiesCACertificateAndRewritesPath() throws {
        try write(appHome.appendingPathComponent("my-ca.pem"), "CA-CERT-BYTES")
        try write(appHome.appendingPathComponent("session.json"), sessionJSON(ca: appHome.appendingPathComponent("my-ca.pem").path))
        try write(appHome.appendingPathComponent("identity.seed"), "seed-bytes")
        XCTAssertTrue(EngineSessionSync.sync(appHome: appHome.path, extHome: extHome.path))
        let extSession = try String(contentsOf: extHome.appendingPathComponent("session.json"), encoding: .utf8)
        // 改写后指向扩展 home 内的拷贝,不再指向 app home 原路径
        XCTAssertTrue(extSession.contains("ca.pem"), "ca 必须改写到扩展 home 内")
        XCTAssertFalse(extSession.contains(appHome.path), "扩展会话不得引用 app home 路径")
        XCTAssertEqual(
            try Data(contentsOf: extHome.appendingPathComponent("ca.pem")),
            Data("CA-CERT-BYTES".utf8))
    }

    /// ca 声明在场但证书文件缺失:同步失败并撤镜像(坏素材不进扩展)。
    func testSyncFailsWhenCACertMissing() throws {
        try write(appHome.appendingPathComponent("session.json"), sessionJSON(ca: "/nonexistent/ca.pem"))
        try write(appHome.appendingPathComponent("identity.seed"), "seed-bytes")
        XCTAssertFalse(EngineSessionSync.sync(appHome: appHome.path, extHome: extHome.path))
        XCTAssertFalse(
            FileManager.default.fileExists(atPath: extHome.appendingPathComponent("session.json").path))
    }

    // MARK: peers.json(TOFU 缓存,尽力拷贝)

    func testSyncCopiesPeersWhenPresentAndToleratesAbsence() throws {
        try write(appHome.appendingPathComponent("session.json"), sessionJSON())
        try write(appHome.appendingPathComponent("identity.seed"), "seed-bytes")
        // 无 peers.json → 同步成功(缺省可再生)
        XCTAssertTrue(EngineSessionSync.sync(appHome: appHome.path, extHome: extHome.path))
        // 有 → 拷贝
        try write(appHome.appendingPathComponent("peers.json"), "{\"peers\":{}}")
        XCTAssertTrue(EngineSessionSync.sync(appHome: appHome.path, extHome: extHome.path))
        XCTAssertEqual(
            try Data(contentsOf: extHome.appendingPathComponent("peers.json")),
            Data("{\"peers\":{}}".utf8))
    }

    // MARK: 扩展 home 派生

    func testExtensionHomeURLUnderGroupRoot() {
        let root = URL(fileURLWithPath: "/group", isDirectory: true)
        let home = EngineSessionSync.extensionHomeURL(groupRoot: root)
        XCTAssertEqual(home.path, "/group/mtdss")
        XCTAssertEqual(EngineSessionSync.extensionHomeName, "mtdss")
    }

    /// UDS 预算钉测(R10 发现:简报 `engine-home` 命名在真机 App Group 容器
    /// 路径下 daemon.sock 超 104 上限——/private/var/mobile/Containers/
    /// Shared/AppGroup/<UUID> = 83B,+「/engine-home/daemon.sock」= 107B 必超;
    /// 「/mtdss/daemon.sock」= 101B 可容。以真机形态字节样例钉死。)
    func testDeviceAppGroupHomeFitsSunPath() {
        let uuid = "12345678-90AB-CDEF-1234-567890ABCDEF"
        let groupRoot = URL(fileURLWithPath:
            "/private/var/mobile/Containers/Shared/AppGroup/\(uuid)", isDirectory: true)
        let layout = EngineLayout(home: EngineSessionSync.extensionHomeURL(groupRoot: groupRoot).path)
        XCTAssertTrue(layout.socketPathsFitSunPath, "真机 App Group home 两 socket 路径必须 < 104:\(layout.daemonSocketPath)(\(layout.daemonSocketPath.utf8.count))")
        // 反面钉:engine-home 命名(简报原案)确实超预算——留此断言防回退
        let over = EngineLayout(home: groupRoot.appendingPathComponent("engine-home", isDirectory: true).path)
        XCTAssertFalse(over.socketPathsFitSunPath)
    }
}
