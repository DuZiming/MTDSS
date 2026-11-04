/// QrApprovalModel — 扫码授权闭环状态机(V3-1 Ruling R8;Kit 可测,App 侧
/// ProfileView 接弹层,ScanSheet 只管取景出 token):
/// - **claim**:扫得 token → qr.claim(daemon 持会话 Bearer 代打)→ 落
///   ``Stage/confirming``(候选面拍平:candidate_name/candidate_platform +
///   expires_at,确认弹层展示);二维码已失效(typed not_found)/他端已认领
///   (contested)/其余错 → ``Stage/failed``(daemon 消息本就面向用户);
/// - **approve**:同意/拒绝 → qr.approve(allow)→ 结果行文案落
///   ``Stage/result``:ok=已同意扫码登录 / denied=已拒绝该设备登录 /
///   expired=二维码已过期,请重新扫码 / limit=对方需选择移除设备后完成登录
///   (limit 为契约全集防御面:配额超额收口在 PC 侧轮询,本 op 恒不投影);
///   RPC 失败 → ``Stage/failed``;
/// - 相位守卫与 LoginModel 同象:approve 仅自 confirming 接受(取消/关闭后
///   的迟到裁决不外发); ``dismiss`` 清场回初始态。
///
/// 所有 RPC 经 ``RpcGuard/withTimeout``(15s:daemon 侧 REST 15s 超时同档)。

import Foundation
import Observation

@MainActor
@Observable
public final class QrApprovalModel {

    /// 单次 RPC 限时(秒;daemon 侧 REST 15s 超时,同档余量;测试注入小值)。
    let callTimeout: TimeInterval

    /// 相位:nil=初始(扫码中)/confirming=候选确认(同意/拒绝)/
    /// result=裁决结果行 / failed=错误文案。
    public enum Stage: Equatable, Sendable {
        case confirming(QrClaimResult)
        case result(String)
        case failed(String)
    }

    public private(set) var stage: Stage?
    public private(set) var busy = false
    /// 确认中的 token(approve 随路携带;claim 成功即上帧)。
    public private(set) var token: String?

    private let repo: MtdssRepository

    public init(repo: MtdssRepository, callTimeout: TimeInterval = 15) {
        self.repo = repo
        self.callTimeout = callTimeout
    }

    // MARK: 动作

    /// qr.claim(扫得 token 后的认领):成功落 confirming;失败落 failed。
    public func claim(token: String) async {
        busy = true
        defer { busy = false }
        do {
            let candidate = try await RpcGuard.withTimeout(seconds: Self.callTimeout) { [repo] in
                try await repo.qrClaim(token: token)
            }
            self.token = token
            stage = .confirming(candidate)
        } catch {
            stage = .failed(Self.userErrorText(for: error))
        }
    }

    /// qr.approve(同意/拒绝):仅自 confirming 接受;结果行文案落 result,
    /// RPC 失败落 failed。
    public func approve(allow: Bool) async {
        guard case .some(.confirming) = stage, let token else { return }
        busy = true
        defer { busy = false }
        do {
            let result = try await RpcGuard.withTimeout(seconds: Self.callTimeout) { [repo] in
                try await repo.qrApprove(token: token, allow: allow)
            }
            stage = .result(Self.resultLine(for: result.status))
        } catch {
            stage = .failed(Self.userErrorText(for: error))
        }
    }

    /// 关闭(弹层 好/取消):清场回初始态(不外发任何 RPC;未裁决的二维码
    /// 由服务端 TTL 自收口)。
    public func dismiss() {
        stage = nil
        token = nil
        busy = false
    }

    // MARK: 文案

    /// 结果行文案(status → 中文;UI 弹层 message 直用)。
    public static func resultLine(for status: String) -> String {
        switch status {
        case "ok": "已同意扫码登录"
        case "denied": "已拒绝该设备登录"
        case "expired": "二维码已过期,请重新扫码"
        case "limit": "对方需选择移除设备后完成登录"
        default: "二维码状态异常"
        }
    }

    /// 错误用户文案(daemon 消息本就面向用户;传输/超时给常识文案)。
    public static func userErrorText(for error: Error) -> String {
        if let rpc = error as? RpcError { return rpc.message }
        if error is RpcGuard.TimeoutError { return "请求超时" }
        return "请求失败:连接中断"
    }
}
