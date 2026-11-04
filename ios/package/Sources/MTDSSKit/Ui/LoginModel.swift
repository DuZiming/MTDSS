/// LoginModel — 登录页视图模型(V3-1 四相位状态机,spec §4.6;android
/// LoginViewModel / 桌面 GuideView 同象):
/// - **Form**:邮箱 → 验证码+「获取验证码」(dev 模式服务器回显码直接预填)
///   → 蓝色大按钮「登录」;高级选项(服务器/CA)小链接展开,缺省烤入
///   www.mtdss.top;
/// - **Approval**:verify 落审批挂起 → 等待受信终端确认(对端弹窗或 6 位
///   审批码直输),轮询 login.status 收口(ok → Done;denied/expired 终态回
///   Form 文案;limit 转场踢选);
/// - **Limit**:verify(或轮询)落配额超额 → 踢选屏(移出所选设备腾位,
///   本机即收口登录);
/// - **Done**:ok 收口(会话已落盘,daemon 广播 event.login → 会话对账),
///   导航翻转以 SessionModel 的 status 为权威,``succeeded`` 只是屏内信号。
///
/// 相位守卫:步进分派只自预期相位接受(verify 自 Form、submit/轮询自
/// Approval、evict 自 Limit)——取消后的迟到步进不拖回旧相位。
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

    /// 登录面相位(Form=邮箱表单;Approval=等待审批;Limit=踢选;Done=收口)。
    public enum Phase: Equatable, Sendable {
        case form
        case approval
        case limit
        case done
    }

    // MARK: 表单(@Observable 输入)

    public var email = ""
    public var code = ""
    /// 高级选项:服务器(缺省烤入)。
    public var server = AccountStore.defaultServer
    /// 高级选项:自签 CA 证书路径("" = 不带 ca 键)。
    public var caPath = ""

    // MARK: 状态(UI 订阅)

    public private(set) var phase: Phase = .form
    public private(set) var busy = false
    public private(set) var errorText: String?
    /// dev 模式服务器回显的验证码(已预填 code;正式服务器为 nil)。
    public private(set) var devCode: String?
    /// verify 成功的一次性屏内信号(onLogin 回调后的屏内反馈;导航权威态
    /// 在 SessionModel)。
    public private(set) var succeeded: LoginVerifyResult?
    /// 轮询终态文案(denied/expired;Form 顶部警示条,重发起时清)。
    public private(set) var terminal: String?

    // Approval 面(等待受信终端确认;6 位审批码直输)
    public private(set) var sessionId: String?
    /// 审批会话到期秒(服务器 TTL;倒计时呈现用,不驱动终态)。
    public private(set) var expiresAt: Int64?
    public var approvalCode = ""

    // Limit 面(踢选凭据 + 服务器设备清单)
    public private(set) var enrollToken: String?
    public private(set) var evictDevices: [DirectoryDevice] = []

    /// 审批轮询节拍(产线 3s;测试注入小值)。
    let pollDelay: TimeInterval
    /// 审批轮询任务(入 Approval 相位起拍,离相位即停)。
    private var pollTask: Task<Void, Never>?

    private let repo: MtdssRepository
    private let account: AccountStore
    /// 平台缺省设备名(App 注入 UIDevice.current.name)。
    public let defaultDeviceName: String
    private let onLogin: (LoginVerifyResult) -> Void
    /// verify 提交的邮箱(ok 收口在任意步进路径,记账随收口走)。
    private var loginEmail: String?

    public init(
        repo: MtdssRepository,
        account: AccountStore,
        defaultDeviceName: String,
        pollDelay: TimeInterval = 3,
        onLogin: @escaping (LoginVerifyResult) -> Void = { _ in },
    ) {
        self.repo = repo
        self.account = account
        self.defaultDeviceName = defaultDeviceName
        self.pollDelay = pollDelay
        self.onLogin = onLogin
        // 预填:上次登录 email + 高级选项记忆(server 缺省烤入;android 同象)
        email = account.email ?? ""
        server = account.server
        caPath = account.caPath
    }

    deinit {
        pollTask?.cancel()
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

    /// login.verify(event.login 由 daemon 收口时广播)→ 步进分派。
    public func verify() async {
        guard phase == .form else { return }
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
        terminal = nil
        loginEmail = mail
        do {
            let name = effectiveDeviceName
            let ca = trimmedCaPath
            let target = trimmedServer
            account.setAdvanced(server: target, caPath: ca)
            let step = try await RpcGuard.withTimeout(seconds: Self.callTimeout) { [repo] in
                try await repo.loginVerify(code: verification, email: mail, name: name)
            }
            applyStep(step, expected: .form)
        } catch {
            errorText = Self.userErrorText(for: error)
            busy = false
        }
    }

    /// login.submit_code(受信终端弹窗上的 6 位审批码直输)。
    public func submitApprovalCode() async {
        guard phase == .approval else { return }
        guard let id = sessionId else { return }
        let codeValue = approvalCode.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !codeValue.isEmpty else {
            errorText = "请填写审批码"
            return
        }
        busy = true
        errorText = nil
        do {
            let step = try await RpcGuard.withTimeout(seconds: Self.callTimeout) { [repo] in
                try await repo.loginSubmitCode(sessionId: id, code: codeValue)
            }
            applyStep(step, expected: .approval)
        } catch {
            if phase == .approval {
                errorText = Self.userErrorText(for: error)
                busy = false
            }
        }
    }

    /// login.evict(踢选屏动作:移出所选设备腾位,本机即收口登录)。
    public func evict(deviceId: String) async {
        guard phase == .limit else { return }
        guard let token = enrollToken else { return }
        busy = true
        errorText = nil
        do {
            let step = try await RpcGuard.withTimeout(seconds: Self.callTimeout) { [repo] in
                try await repo.loginEvict(enrollToken: token, deviceId: deviceId)
            }
            applyStep(step, expected: .limit)
        } catch {
            if phase == .limit {
                errorText = Self.userErrorText(for: error)
                busy = false
            }
        }
    }

    /// 取消(任一中间态 → Form):本地先回退(相位清场),login.cancel 随后
    /// 作废 daemon 续接槽;服务端会话无取消端点,TTL 自收口,失败不阻回退。
    public func cancel() {
        guard phase != .form else { return }
        stopPolling()
        phase = .form
        sessionId = nil
        expiresAt = nil
        approvalCode = ""
        enrollToken = nil
        evictDevices = []
        busy = false
        errorText = nil
        terminal = nil
        let repo = self.repo
        Task { [repo] in
            try? await RpcGuard.withTimeout(seconds: Self.callTimeout) { [repo] in
                try await repo.loginCancel()
            }
        }
    }

    // MARK: 步进分派

    /**
     * 步进分派收口(ok → Done;approval → 等待面+轮询;limit → 踢选面;
     * denied/expired → 终态回 Form 文案)。仅自 ``expected`` 相位接受——
     * 取消后的迟到步进(轮询在飞)原样丢弃。
     */
    private func applyStep(_ step: LoginStep, expected: Phase) {
        guard phase == expected else { return }
        switch step {
        case .ok(let result):
            stopPolling()
            phase = .done
            busy = false
            errorText = nil
            sessionId = nil
            expiresAt = nil
            approvalCode = ""
            enrollToken = nil
            evictDevices = []
            // 记账 + 屏内信号(邮箱取 verify 提交值;实名以引擎应答为准,
            // 槽内无候选名回退生效名)
            let mail = loginEmail ?? account.email ?? ""
            let name = result.name ?? effectiveDeviceName
            account.recordLogin(email: mail, name: name)
            succeeded = result
            onLogin(result)
        case .approval(let id, let expires):
            busy = false
            phase = .approval
            sessionId = id
            expiresAt = expires
            errorText = nil
            startPolling(id)
        case .limit(let token, let devices):
            // verify 直达或轮询/审批转场都落此(evict 再撞 limit 同此换新)
            stopPolling()
            busy = false
            phase = .limit
            enrollToken = token
            evictDevices = devices
            errorText = nil
        case .pending:
            // 审批面无 pending 形态(qr 专用,V3-4);保守不动相位
            break
        case .denied:
            toTerminal("登录请求已被拒绝")
        case .expired:
            toTerminal("登录已超时,请重新发起")
        }
    }

    /// 终态(denied/expired):回 Form 顶部警示文案,中间态凭据清场。
    private func toTerminal(_ message: String) {
        stopPolling()
        busy = false
        phase = .form
        terminal = message
        sessionId = nil
        expiresAt = nil
        approvalCode = ""
        enrollToken = nil
        evictDevices = []
    }

    // MARK: 内部

    /**
     * 审批轮询(服务端 push 不达新终端,收口靠本面):每拍 ``pollDelay``
     * 一次 login.status,步进自 Approval 相位分派;单拍失败(瞬态网络/引擎
     * 重启)不断轮询,终态由服务端 TTL 收口。相位离 Approval 即停。
     */
    private func startPolling(_ id: String) {
        stopPolling()
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: UInt64((self?.pollDelay ?? 3) * 1_000_000_000))
                guard let self, !Task.isCancelled else { return }
                await self.pollBeat(sessionId: id)
            }
        }
    }

    /// 轮询单拍:失败静默续轮;步进交 applyStep(相位守卫拒迟到)。
    private func pollBeat(sessionId id: String) async {
        let step = try? await RpcGuard.withTimeout(seconds: Self.callTimeout) { [repo] in
            try await repo.loginStatus(sessionId: id)
        }
        if let step {
            applyStep(step, expected: .approval)
        }
    }

    private func stopPolling() {
        pollTask?.cancel()
        pollTask = nil
    }

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
