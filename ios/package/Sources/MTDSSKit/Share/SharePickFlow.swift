/// 分享点选流状态机(R6 #4a;Kit 可测,绑定在扩展 ``SharePanelView``):
///
///     idle ──pick(d)──▶ busy(d) ──openSettled──▶ launched(d)(拉起成功;
///        ▲                 │                        短暂「已发送」后收口)
///        │                 │ 入队失败                 │
///        │           enqueueFailed               (视图 onDone → 扩展收口)
///        │                 ▼                        ▲
///        ├─── pick ─── failed(msg)                  │ open 失败/超时 + 绕法成功
///        │                                          │
///        └─────── pick ◀── queued(d) ◀── openSettled ┘ open 失败/超时 + 绕法不通
///                 (再选)      「已排队,打开 MTDSS 即自动发送」注记
///
/// 点选即忙(R6 用户报告:点设备后数秒无可见反应——busy 态给行内 spinner +
/// 整表禁点,3s open() 竞窗全程有反馈);queued/failed 均可再 pick(换设备
/// 重新入队,与 T11 队列语义一致——一队一项,重选即新增一项)。
public struct SharePickFlow: Equatable, Sendable {
    /// 流相位(idle=可点选;busy=点选后 open 竞窗进行中;launched=拉起
    /// 成功;queued=降级注记;failed=入队失败)。
    public enum Phase: Equatable, Sendable {
        case idle
        case busy(deviceId: String)
        case launched(deviceId: String)
        case queued(deviceId: String)
        case failed(message: String)
    }

    public private(set) var phase: Phase = .idle

    public init() {}

    /// 点选设备(idle/queued/failed → busy)。busy/launched 中拒点(返回
    /// false;视图侧另有整表禁点双保险)。
    @discardableResult
    public mutating func pick(deviceId: String) -> Bool {
        switch phase {
        case .idle, .queued, .failed:
            phase = .busy(deviceId: deviceId)
            return true
        case .busy, .launched:
            return false
        }
    }

    /// 入队失败(busy → failed;消息入文案,可再 pick 重选)。
    public mutating func enqueueFailed(message: String) {
        guard case .busy = phase else { return }
        phase = .failed(message: message)
    }

    /// open 竞窗收口(busy → launched/queued):`opened` = 官方
    /// NSExtensionContext.open 回话;`fallbackOpened` = open 未通时
    /// responder-chain 绕法的结果(spec §4.7 备选;二者任一真 = launched)。
    public mutating func openSettled(opened: Bool, fallbackOpened: Bool) {
        guard case .busy(let deviceId) = phase else { return }
        if opened || fallbackOpened {
            phase = .launched(deviceId: deviceId)
        } else {
            phase = .queued(deviceId: deviceId)
        }
    }
}
