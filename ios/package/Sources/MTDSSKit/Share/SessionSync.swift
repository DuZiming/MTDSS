/// 会话同步(R10,主 App → 扩展):主 App 引擎 home 的**最小登录素材**
/// 原子镜像进扩展引擎 home(App Group)——扩展引擎从该 home 启动即同设备
/// 身份、已登录(brief R10 架构 ②)。
///
/// **最小素材集(契约事实,crates/daemon)**:
/// - `session.json`(server/jwt/refresh/device_id/ca)——登录态唯一载体
///   (state.rs:session.json 存在且可解析 = 已登录;hub.rs:401 时 refresh
///   自愈轮换写回本 home 的 session.json);
/// - `identity.seed`——本机 Ed25519 身份;flow.rs:与 session.json 的
///   device_id 分歧即硬错 → **两者必须成对**,缺一即整单不同步(防「旧
///   session + 引擎新生成 seed」的启动硬错);
/// - `ca`(session 内为**绝对路径**,自签场景)→ 拷证书入扩展 home 并改写
///   指向(两沙箱不互通,扩展读不到 App 容器路径);
/// - `peers.json`(TOFU 缓存)缺省可再生 → 尽力拷贝,缺席不算失败。
/// **refresh 轮换是无状态 JWT 重铸**(server auth.rs,无吊销注册表)→
/// 两份副本各自自愈,无轮换分歧风险(R10 契约核查结论)。
///
/// **原子性**:逐文件临时名+rename;`session.json` 恒最后落(提交标记——
/// 扩展侧见 session.json 在场 = 其余素材已就位);撤镜像(登出/素材不全)
/// 先删 session.json 再清其余。
///
/// 纯 Foundation,宿主可测(SessionSyncTests);主 App 侧绑定在
/// MTDSSApp(boot/event.login/前台回归三触发)。
import Foundation

public enum EngineSessionSync {

    /// 扩展引擎 home 目录名(App Group 容器下)。**不是**简报原案的
    /// `engine-home`:真机 App Group 容器路径
    /// `/private/var/mobile/Containers/Shared/AppGroup/<UUID>` 已 83 字节,
    /// + `/engine-home/daemon.sock`(24)= 107 字节,超 `sun_path` 104
    /// 上限(UDS bind 必败);`/mtdss/daemon.sock` = 101 字节可容
    /// (SessionSyncTests 以真机字节样例钉死,含反面钉)。
    public static let extensionHomeName = "mtdss"

    /// 扩展引擎 home 派生:`{App Group 容器}/mtdss`。
    public static func extensionHomeURL(groupRoot: URL) -> URL {
        groupRoot.appendingPathComponent(extensionHomeName, isDirectory: true)
    }

    /// 文件名(与 daemon home 布局逐字对照;公开供扩展侧在场性检查复用)。
    public static let sessionFile = "session.json"
    public static let identityFile = "identity.seed"
    static let peersFile = "peers.json"
    /// ca 证书在扩展 home 内的拷贝名(改写后的指向)。
    static let caFile = "ca.pem"

    /// 同步一轮(幂等):app home 会话素材 → ext home。
    /// 返回 true = 扩展素材就绪(可直发);false = 未登录/素材不全/拷贝
    /// 失败(扩展侧据此降级队列)。**false 时扩展 home 不留登录素材**。
    @discardableResult
    public static func sync(
        appHome: String, extHome: String, fileManager: FileManager = .default,
    ) -> Bool {
        let app = URL(fileURLWithPath: appHome, isDirectory: true)
        let ext = URL(fileURLWithPath: extHome, isDirectory: true)
        do {
            try fileManager.createDirectory(at: ext, withIntermediateDirectories: true)
            let sessionData = try Data(contentsOf: app.appendingPathComponent(sessionFile))
            let identityData = try Data(contentsOf: app.appendingPathComponent(identityFile))
            // ca 改写:解析 session JSON,ca 在场则拷证书并改写指向(拷贝
            // 失败 = 素材不全,整单不同步)。
            var session = sessionData
            if var obj = try JSONSerialization.jsonObject(with: sessionData) as? [String: Any],
               let caPath = obj["ca"] as? String, !caPath.isEmpty {
                let caData = try Data(contentsOf: URL(fileURLWithPath: caPath))
                try writeAtomic(caData, to: ext.appendingPathComponent(caFile), fileManager: fileManager)
                obj["ca"] = ext.appendingPathComponent(caFile).path
                session = try JSONSerialization.data(withJSONObject: obj)
            }
            // 提交序:identity/peers 先,session 恒最后(提交标记)。
            try writeAtomic(identityData, to: ext.appendingPathComponent(identityFile), fileManager: fileManager)
            let peersSrc = app.appendingPathComponent(peersFile)
            if fileManager.fileExists(atPath: peersSrc.path) {
                try? writeAtomic(
                    Data(contentsOf: peersSrc),
                    to: ext.appendingPathComponent(peersFile), fileManager: fileManager)
            }
            try writeAtomic(session, to: ext.appendingPathComponent(sessionFile), fileManager: fileManager)
            return true
        } catch {
            // 未登录(session 缺)/素材不全(identity 缺)/ca 读失败/盘错
            // → 撤镜像(先 session 后其余),扩展侧降级队列。
            clear(ext: ext, fileManager: fileManager)
            return false
        }
    }

    /// 撤镜像(登出/素材不全):先删提交标记(session.json)再清其余——
    /// 扩展侧「session 在场 = 素材齐」不变式不破。
    static func clear(ext: URL, fileManager: FileManager) {
        // 先 session(提交标记),后 identity/ca/peers
        for name in [sessionFile, identityFile, caFile, peersFile] {
            try? fileManager.removeItem(at: ext.appendingPathComponent(name))
        }
    }

    /// 原子写:临时名(`.tmp`)写入 + 同目录 rename(读者见旧或见新,
    /// 不见半截;Unix 上落 0600,与 daemon `write_private` 同口径)。
    private static func writeAtomic(
        _ data: Data, to url: URL, fileManager: FileManager,
    ) throws {
        let tmp = url.appendingPathExtension("tmp")
        try data.write(to: tmp, options: .atomic)
        #if canImport(Darwin)
        try? fileManager.setAttributes([.posixPermissions: 0o600], ofItemAtPath: tmp.path)
        #endif
        if fileManager.fileExists(atPath: url.path) {
            try fileManager.removeItem(at: url)
        }
        try fileManager.moveItem(at: tmp, to: url)
    }
}
