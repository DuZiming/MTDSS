/// AccountStore — App 侧账号档案(UserDefaults;T10):
/// 引擎 session.json 无 email/本机名面(契约事实:status.device.name 恒
/// null,名字只在 login.verify 提交且回显;无运行时改名 RPC/REST)——
/// 「我的」账号行、登录页注脚缺省、下次 verify 的 name 皆取此处:
/// - 登录成功(verify)→ ``recordLogin(email:name:)`` 记账(实名以引擎
///   应答为准);
/// - 设置页「我的设备名」改名 → ``setDeviceNameOverride``(**下次登录对端
///   生效**,daemon 对同 pubkey 幂等 upsert;运行时无改名面,UI 注脚如实);
/// - 登录页高级选项(server/CA)记忆 → ``setAdvanced``(android
///   SharedPreferences 预填同象;server 缺省烤入 www.mtdss.top)。

import Foundation

@MainActor
public final class AccountStore: ObservableObject {

    /// 登录页高级选项缺省(spec §4.6:默认烤入 www.mtdss.top)。
    public static let defaultServer = "https://www.mtdss.top"

    private enum Key {
        static let email = "mtdss.account.email"
        static let deviceName = "mtdss.account.deviceName"
        static let deviceNameOverride = "mtdss.account.deviceNameOverride"
        static let server = "mtdss.account.server"
        static let caPath = "mtdss.account.caPath"
    }

    /// 底层存储(测试以 suite 隔离注入;跨实例重建可验持久化)。
    public let underlyingDefaults: UserDefaults

    /// 账号 email(登录成功才有;引擎不回供)。
    @Published public private(set) var email: String?
    /// 最近一次登录的实名(login.verify 应答 name)。
    @Published public private(set) var deviceName: String?
    /// 设置页改名(优先于实名;下次登录上帧)。
    @Published public private(set) var deviceNameOverride: String?
    /// 高级选项:服务器(缺省烤入)。
    @Published public private(set) var server: String
    /// 高级选项:自签 CA 证书路径("" = 不带 ca 键)。
    @Published public private(set) var caPath: String

    public init(defaults: UserDefaults) {
        self.underlyingDefaults = defaults
        email = defaults.string(forKey: Key.email)
        deviceName = defaults.string(forKey: Key.deviceName)
        deviceNameOverride = defaults.string(forKey: Key.deviceNameOverride)
        server = defaults.string(forKey: Key.server) ?? Self.defaultServer
        caPath = defaults.string(forKey: Key.caPath) ?? ""
    }

    /// 登录成功记账(实名以 verify 应答为准;缺省名注入时回写使注脚一致)。
    public func recordLogin(email: String, name: String) {
        self.email = email
        deviceName = name
        underlyingDefaults.set(email, forKey: Key.email)
        underlyingDefaults.set(name, forKey: Key.deviceName)
    }

    /// 设置页改名(nil = 撤回改回实名);下次 login.verify 生效。
    public func setDeviceNameOverride(_ name: String?) {
        deviceNameOverride = name
        if let name {
            underlyingDefaults.set(name, forKey: Key.deviceNameOverride)
        } else {
            underlyingDefaults.removeObject(forKey: Key.deviceNameOverride)
        }
    }

    /// 高级选项记忆(获取验证码/登录时随表单值保存)。
    public func setAdvanced(server: String, caPath: String) {
        self.server = server
        self.caPath = caPath
        underlyingDefaults.set(server, forKey: Key.server)
        underlyingDefaults.set(caPath, forKey: Key.caPath)
    }

    /// 生效设备名:设置改名 > 登录实名 > 平台缺省(UIDevice.current.name 注入)。
    public func effectiveDeviceName(default fallback: String) -> String {
        deviceNameOverride ?? deviceName ?? fallback
    }
}
