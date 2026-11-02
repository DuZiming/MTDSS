/// 分享扩展主体(T11,spec §4.7/§5.4;NSExtensionPrincipalClass):
/// - 壳 = UIViewController + UIHostingController(SharePanelView,SwiftUI
///   面板;扩展目标不链引擎,内存限额 + 秒级寿命硬约束);
/// - open() 拉起宿主两级(T11 + R6 #4b):
///   1) 官方 NSExtensionContext.open(iOS 26 真机实证不拉起不回调——
///      20261102.4 E4 fork b;3s 竞窗兜回调缺失);
///   2) **responder-chain 绕法**(spec §4.7 钦定备选):
///      ⚠️ 审核风险注:自扩展 principal VC 沿 responder 链上溯至进程内
///      UIApplication 对象,经 ObjC `openURL:` legacy 选择器拉起 mtdss://
///      (scheme 已注册)——消息走运行时转发,非扩展白名单面。仅在官方
///      open() 明确失败/超时后作为**降级路径**使用,不主动直走;两条路
///      均不通 = 「已排队,打开 MTDSS 即自动发送」注记(队列消费在主 App
///      前台/登录触发)。
/// - completeRequest 收口:返回空 items(文件已拷入 App Group 队列,
///   不回传宿主)。
/// 全程 os_log(R6:真机轮排障;Console 按 subsystem top.mtdss.ios.share)。

import SwiftUI
import UIKit
import MTDSSKit
import os

private let shareVcLog = Logger(subsystem: "top.mtdss.ios.share", category: "vc")

/// responder-chain 拉起面(@objc 协议;UIApplication 响应 openURL:,
/// 扩展目标不直引 UIApplication API——NS_EXTENSION_UNAVAILABLE 编译门)。
@objc protocol URLOpening {
    @objc(openURL:)
    func open(_ url: URL) -> Bool
}

final class ShareViewController: UIViewController {

    private let writer = ShareQueueWriter()

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .clear
        shareVcLog.info("extension viewDidLoad")
        let panel = SharePanelView(
            writer: writer,
            context: extensionContext ?? NSExtensionContext(),
            openHost: { [weak self] url in
                // 拉起尝试(评审 I-1/I-2):ctx.open 须主线程(MainActor.run
                // 包裹发起);等回话走取消感知续体(Kit withCallbackFallback
                // ——completionHandler 部分系统形态永不回调,竞窗超时靠取消
                // 收口;迟到回调被恰一次守卫吞掉)
                guard let ctx = self?.extensionContext else { return false }
                return await withCallbackFallback { onDone in
                    await MainActor.run {
                        ctx.open(url) { success in
                            shareVcLog.info("ctx.open callback success=\(success)")
                            onDone(success)
                        }
                    }
                }
            },
            openHostFallback: { [weak self] url in
                self?.openViaResponderChain(url) ?? false
            },
            onDone: { [weak self] in self?.finish() },
        )
        let hosting = UIHostingController(rootView: panel)
        hosting.view.backgroundColor = .clear
        hosting.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        addChild(hosting)
        view.addSubview(hosting.view)
        hosting.view.frame = view.bounds
        hosting.didMove(toParent: self)
    }

    /// responder-chain 绕法(R6 #4b;spec §4.7 备选,审核风险注见类型头):
    /// 自 VC 沿 next 上溯,命中响应 `openURL:` 的对象(UIApplication)即发。
    /// 返回 openURL: 的 Bool(发起成功与否;真拉起效果真机轮验证)。
    private func openViaResponderChain(_ url: URL) -> Bool {
        var responder: UIResponder? = self
        var depth = 0
        while let current = responder {
            if let opener = current as? URLOpening {
                let opened = opener.open(url)
                shareVcLog.info("responder-chain hit at depth=\(depth) -> \(opened)")
                return opened
            }
            responder = current.next
            depth += 1
        }
        shareVcLog.info("responder-chain no opener found")
        return false
    }

    /// 收口(入队成功后;open 成功即刻 / 降级注记由用户「好」触发)。
    /// completeRequest 后扩展进程随时可被系统回收——入队先于本口完成
    /// (两步拷贝 + meta 翻 ready),队列状态落盘为准。
    private func finish() {
        shareVcLog.info("completeRequest")
        extensionContext?.completeRequest(returningItems: nil, completionHandler: nil)
    }
}
