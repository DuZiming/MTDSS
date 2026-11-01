/// SessionModel — 会话状态模型(T10,我的/登录页的状态面;android
/// SessionViewModel 同象):
/// - **status 全量刷新**(进页/动作后):logged_in / online / device 投影;
/// - **event.login 增量翻转**:登录成功 true、登出(幂等)false 都广播
///   (state.rs:381/407)——App 事件泵逐帧喂 ``consume``;false 收敛清空
///   device_id(会话态不留尸);
/// - **登出链**:login.logout RPC(幂等,daemon 契约)+ 乐观清空;事件
///   false 随后重申,迟到 true 不复活已清会话(App 层以 status 全量刷新
///   收口权威态)。
/// 限时:status 5s(T5 AppModel 同口径)/ logout 10s。

import Foundation
import Observation

@MainActor
@Observable
public final class SessionModel {

    /// 登录态(status 权威;事件翻转;未知 = nil 引擎未就绪)。
    public private(set) var loggedIn: Bool?
    /// 本机 device_id(未登录/已登出为 nil)。
    public private(set) var deviceId: String?
    /// 信令在线(status.online)。
    public private(set) var online: Bool?

    public init() {}

    /// status 全量刷新(限时;失败保留旧值——bootError 横幅另有兜底)。
    public func refresh(repo: MtdssRepository) async {
        do {
            let status = try await RpcGuard.withTimeout(seconds: 5) { [repo] in
                try await repo.status()
            }
            loggedIn = status.loggedIn
            online = status.online
            deviceId = status.device?.deviceId
        } catch {
            // 保留旧值(T5 refreshStatus 同口径)
        }
    }

    /// 登录成功翻转(LoginModel onSuccess → App 装配喂入;权威态仍以
    /// status 刷新收口)。
    public func applyLogin(_ result: LoginVerifyResult) {
        loggedIn = true
        deviceId = result.deviceId
    }

    /// 事件泵喂入(event.login/online 翻转;其余事件不属会话面)。
    public func consume(_ event: IpcEvent) {
        switch event {
        case .login(let loggedInNow):
            loggedIn = loggedInNow
            if !loggedInNow {
                deviceId = nil
            }
        case .online(let onlineNow):
            online = onlineNow
        default:
            break
        }
    }

    /// 登出:RPC(幂等成功)+ 乐观清空;event.login(false) 收敛重申。
    public func logout(repo: MtdssRepository) async throws {
        try await RpcGuard.withTimeout(seconds: 10) { [repo] in
            try await repo.loginLogout()
        }
        loggedIn = false
        deviceId = nil
    }
}
