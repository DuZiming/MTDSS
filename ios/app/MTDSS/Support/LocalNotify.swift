/// 本地通知 App 侧实现(T9,spec §3「传输完成 → 发本地通知」;无推送):
/// - ``UNLocalNotifier``:Kit ``LocalNotifier`` 协议的 UNUserNotificationCenter
///   实现。**授权时机(已裁定,least intrusive)= 首次真正要发通知时**请求
///   一次(用户在完成瞬间被请求,语境最自然;拒绝则静默跳过,不再骚扰);
/// - 前台呈现:iOS 默认前台静默投递——delegate `willPresent` 交系统
///   banner+sound(与后台同一呈现语言,零自绘);Kit/宿主测试不触碰本文件
///   (Kit 面 = `NoopLocalNotifier`),通知权限零依赖(Ruling E:逻辑归
///   Kit,绑定归 App)。

import Foundation
import MTDSSKit
import UserNotifications

final class UNLocalNotifier: LocalNotifier, @unchecked Sendable {
    private let center = UNUserNotificationCenter.current()
    /// 授权请求恰一次闸(nil = 未请求;负结果同缓存——拒绝后不再骚扰)。
    private let lock = NSLock()
    private var authorized: Bool?

    init() {
        // 前台呈现闸delegate(常驻单例,随 notifier 生命周期存活)
        center.delegate = NotifyDelegateBox.shared
    }

    func notify(title: String, body: String) async {
        guard await ensureAuthorized() else { return }
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.sound = .default
        let request = UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil)
        try? await center.add(request)
    }

    /// 授权判定:notDetermined → 请求一次(alert+sound;完成横幅所需最小集),
    /// 已授权/临时授权 → true,拒绝/其他 → false。结果缓存,会话内不再请求。
    private func ensureAuthorized() async -> Bool {
        lock.lock()
        let cached = authorized
        lock.unlock()
        if let cached { return cached }
        let settings = await center.notificationSettings()
        let grantedNow: Bool
        switch settings.authorizationStatus {
        case .notDetermined:
            grantedNow = (try? await center.requestAuthorization(options: [.alert, .sound])) ?? false
        case .authorized, .provisional, .ephemeral:
            grantedNow = true
        default:
            grantedNow = false
        }
        lock.lock()
        authorized = grantedNow
        lock.unlock()
        return grantedNow
    }
}

/// 前台呈现 delegate(iOS 默认前台不弹横幅;完成通知前后台同语言呈现)。
private final class NotifyDelegateBox: NSObject, UNUserNotificationCenterDelegate, @unchecked Sendable {
    static let shared = NotifyDelegateBox()

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
    ) async -> UNNotificationPresentationOptions {
        [.banner, .sound]
    }
}
