/// NavigationStack 值路径转场重入闸(§13.7 崩溃修复,2026-11-06)。
///
/// **根因**(sim 实证,.ips 在案):iOS 26 的 ``NavigationStack`` 值路径在
/// pop/push **转场在飞窗**(≈0.35s 动画)内再次变更时,SwiftUI 内部
/// `NavigationColumnState.boundPathChange` 断言即炸(SIGTRAP,非可捕获)。
/// 触达形态 = 快捷目录页点行 → 秒退 → 立刻再点(用户报告「点行进不去即
/// 崩,成员离线窗+连点」;成员离线只拉长转场窗内的可视窗,连点才崩)。
/// 四个值路由栈(快捷首页/混合页钻取/设备/我的)同一暴露面。
///
/// **收口**:全部值路径写入经 ``write(_:to:)`` 单入口——
/// - 同值写入零操作(`NavigationStack` 对等值本无操作,显式跳过省一次
///   diff);
/// - 转场窗内的异值写入**不直落 binding**(直落即重入崩面),记为窗尾
///   待办(最后写入胜),窗尾统一套用——导航意图只延迟不丢弃;
/// - 直落写入开启 0.45s 窗(0.35s 转场 + 余量)。
/// 宿主视图侧保持既有语义:写「目标整栈」(append 场景 = 现栈 + 新页)。
///
/// 纯 SwiftUI 侧组件(Kit 无 SwiftUI 依赖,故落 App 目标 Common);无
/// @Published 面——簿记态不驱动视图,宿主 @State(binding)才驱动。

import SwiftUI

/// 转场窗时长:push/pop 动画 ≈0.35s + 余量(窗内再写入一律延迟套用)。
/// 泛型类型不持 static 存储属性(Swift 限制),置文件级常量。
private let navPathGateWindow: TimeInterval = 0.45

@MainActor
final class NavPathGate<Route: Equatable> {

    private var windowUntil = Date.distantPast
    /// 窗内待办(最后写入胜;窗尾统一套用)。
    private var pending: [Route]?
    /// 恰一个在飞的窗尾套用计时(窗内多次写入只排一次)。
    private var flushTask: Task<Void, Never>?

    /// 值路径写入入口。`path` = 宿主页 NavigationStack 的 path @State 投影。
    func write(_ target: [Route], to path: Binding<[Route]>) {
        let now = Date()
        if now < windowUntil {
            // 转场在飞:不直落 binding(重入崩面),记窗尾待办(最后写入胜)
            pending = target
            scheduleFlush(path)
            return
        }
        guard path.wrappedValue != target else { return } // 同值零操作
        path.wrappedValue = target
        windowUntil = now.addingTimeInterval(navPathGateWindow)
    }

    /// 窗尾统一套用待办;套用时窗已过 → 走直落分支(开启新窗)。
    private func scheduleFlush(_ path: Binding<[Route]>) {
        guard flushTask == nil else { return }
        let wait = max(0, windowUntil.timeIntervalSinceNow) + 0.03
        flushTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(wait * 1_000_000_000))
            guard let self else { return }
            self.flushTask = nil
            guard let target = self.pending else { return }
            self.pending = nil
            self.write(target, to: path)
        }
    }
}
