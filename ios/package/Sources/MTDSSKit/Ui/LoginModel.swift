/// LoginModel — 登录页视图模型(T10,spec §4.6;android LoginViewModel 同象):
/// 邮箱 → 验证码+「获取验证码」(dev 模式服务器回显码直接预填,桌面同款
/// 便利)→ 蓝色大按钮「登录」;高级选项(服务器/CA)小链接展开,缺省烤入
/// www.mtdss.top。
/// - 生效设备名 = AccountStore 改名 > 登录实名 > 平台缺省注入
///   (UIDevice.current.name;Kit 不 import UIKit);verify 上帧即契约
///   `login.verify {code, email, name}` 的 name;
/// - 导航权威态在 SessionModel(App 装配经 onLogin 喂 applyLogin);
/// - 所有 RPC 经 ``RpcGuard/withTimeout``(30s:服务器 HTTP 段最重)。

import Foundation
import Observation

@MainActor
@Observable
public final class LoginModel {

    /// 单次 RPC 限时(秒;request_code/verify 走服务器 HTTP,daemon 侧 15s
    /// 超时,取 BrowseViewModel.callTimeout 同档余量)。
    static let callTimeout: TimeInterval = 30

    // MARK: 表单(@Published 输入)

    public var email = ""
    public var code = ""
    /// 高级选项:服务器(缺省烤入)。
    public var server = AccountStore.defaultServer
    /// 高级选项:自签 CA 证书路径("" = 不带 ca 键)。
    public var caPath = ""

    // MARK: 状态(UI 订阅)

    public private(set) var busy = false
    public private(set) var errorText: String?
    /// dev 模式服务器回显的验证码(已预填 code;正式服务器为 nil)。
    public private(set) var devCode: String?
    /// verify 成功的一次性屏内信号(onLogin 回调后的屏内反馈)。
    public private(set) var succeeded: LoginVerifyResult?

    private let repo: MtdssRepository
    private let account: AccountStore
    /// 平台缺省设备名(App 注入 UIDevice.current.name)。
    public let defaultDeviceName: String
    private let onLogin: (LoginVerifyResult) -> Void

    public init(
        repo: MtdssRepository,
        account: AccountStore,
        defaultDeviceName: String,
        onLogin: @escaping (LoginVerifyResult) -> Void = { _ in },
    ) {
        self.repo = repo
        self.account = account
        self.defaultDeviceName = defaultDeviceName
        self.onLogin = onLogin
        // 预填:上次登录 email + 高级选项记忆(server 缺省烤入;android 同象)
        email = account.email ?? ""
        server = account.server
        caPath = account.caPath
    }

    /// 生效设备名(注脚与 verify 同源)。
    public var effectiveDeviceName: String {
        account.effectiveDeviceName(default: defaultDeviceName)
    }

    /// 登录按钮可用面(邮箱+验证码齐备且不在飞)。
    public var canSubmit: Bool {
        !trimmedEmail.isEmpty && !trimmedCode.isEmpty && !busy
    }

    // MARK: 动作

    /// login.request_code:空字段本地校验失败(不触 RPC);成功预填回显码。
    public func requestCode() async {
        let target = trimmedServer
        let mail = trimmedEmail
        guard !target.isEmpty, !mail.isEmpty else {
            errorText = "请填写服务器地址与邮箱"
            return
        }
        busy = true
        errorText = nil
        defer { busy = false }
        do {
            let ca = trimmedCaPath
            let result = try await RpcGuard.withTimeout(seconds: Self.callTimeout) { [repo] in
                try await repo.loginRequestCode(server: target, email: mail, ca: ca.isEmpty ? nil : ca)
            }
            account.setAdvanced(server: target, caPath: ca)
            // dev 回显直接预填(正式服务器 code 为空则不动用户输入)
            if !result.code.isEmpty {
                devCode = result.code
                code = result.code
            }
        } catch {
            errorText = Self.userErrorText(for: error)
        }
    }

    /// login.verify:成功 → 账本记账(email/实名)+ onLogin 回调(权威态)。
    public func verify() async {
        let mail = trimmedEmail
        let verification = trimmedCode
        guard !mail.isEmpty else {
            errorText = "请填写邮箱"
            return
        }
        guard !verification.isEmpty else {
            errorText = "请填写验证码"
            return
        }
        busy = true
        errorText = nil
        defer { busy = false }
        do {
            let name = effectiveDeviceName
            let ca = trimmedCaPath
            let target = trimmedServer
            account.setAdvanced(server: target, caPath: ca)
            let result = try await RpcGuard.withTimeout(seconds: Self.callTimeout) { [repo] in
                try await repo.loginVerify(code: verification, email: mail, name: name)
            }
            account.recordLogin(email: mail, name: result.name)
            succeeded = result
            onLogin(result)
        } catch {
            errorText = Self.userErrorText(for: error)
        }
    }

    // MARK: 内部

    private var trimmedEmail: String { email.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var trimmedCode: String { code.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var trimmedServer: String { server.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var trimmedCaPath: String { caPath.trimmingCharacters(in: .whitespacesAndNewlines) }

    /// 错误用户文案(daemon 消息本就面向用户;传输/超时给常识文案)。
    public static func userErrorText(for error: Error) -> String {
        if let rpc = error as? RpcError { return rpc.message }
        if error is RpcGuard.TimeoutError { return "请求超时" }
        return "请求失败:连接中断"
    }
}
