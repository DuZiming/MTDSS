/// 本地通知小面(spec §3「传输完成 → 发本地通知」;无推送,系统本地通知):
/// - ``LocalNotifier`` 协议 + no-op 缺省(Kit 只持抽象与判定面;UN 系统实现
///   在 App 目标 `Support/LocalNotify.swift`——host 测试不触碰通知权限,
///   Ruling E 同口径:逻辑归 Kit 可测,绑定归 App);
/// - ``TransferCompletionAnnouncer``:传输事件流 → 完成沿判定。触发条件
///   (TransfersViewModelTests 钉死):**仅 done、每作业每会话恰一次**——
///   进行中/失败/取消不通知(spec §3 只裁完成);去重集会话内有效,
///   job_id 每任务唯一(reinitiate 翻新换新 id),重启自然重置。
///
/// 授权时机(least intrusive,已裁定):**首次真正要发通知时**请求一次
/// (`UNLocalNotifier` 内实现);用户拒绝则静默跳过,不再骚扰。

import Foundation

/// 本地通知发送抽象(App 目标注入 UNUserNotificationCenter 实现)。
public protocol LocalNotifier: Sendable {
    /// 发一条本地通知(实现方自管授权请求;未授权静默跳过)。
    func notify(title: String, body: String) async
}

/// no-op 缺省(测试/未接/App 扩展场景)。
public struct NoopLocalNotifier: LocalNotifier {
    public init() {}
    public func notify(title: String, body: String) async {}
}

/// 传输完成通知判定面(纯逻辑,宿主可测):event.transfer 逐帧喂入,
/// 首个 done 帧回传该帧(调用方据此组文案),其余(进行中/失败/取消/
/// 重复 done)回 nil。
public struct TransferCompletionAnnouncer: Sendable {
    private var notified: Set<String> = []

    public init() {}

    /// 判定一帧;done 且本会话未通知过 → 回传该帧,否则 nil。
    public mutating func announce(_ p: TransferProgress) -> TransferProgress? {
        guard p.state == "done", !notified.contains(p.id) else { return nil }
        notified.insert(p.id)
        return p
    }

    /// 重对账收口(前台回归 transfer.list 快照):快照里已 done 的作业记入
    /// 去重集——竞窗内错过的事件帧不再由迟到通道补发通知(历史完成不打扰)。
    public mutating func reconcile(_ jobs: [TransferJob]) {
        for job in jobs where job.state == "done" {
            notified.insert(job.id)
        }
    }
}
