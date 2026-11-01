/// 分享扩展主体(T11,spec §4.7/§5.4;NSExtensionPrincipalClass):
/// - 壳 = UIViewController + UIHostingController(SharePanelView,SwiftUI
///   面板;扩展目标不链引擎,内存限额 + 秒级寿命硬约束);
/// - open() 拉起宿主尝试(NSExtensionContext.open;iOS 26 实际行为真机
///   T13 验证——官方对 share extension 拉起宿主有限制,responder-chain
///   绕法带审核风险(spec 4.7 注)v1 不做;失败降级「已排队,打开 App
///   即自动发送」,主 App 前台/登录触发消费);
/// - completeRequest 收口:返回空 items(文件已拷入 App Group 队列,
///   不回传宿主)。

import SwiftUI
import UIKit
import MTDSSKit

final class ShareViewController: UIViewController {

    private let writer = ShareQueueWriter()

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .clear
        let panel = SharePanelView(
            writer: writer,
            context: extensionContext ?? NSExtensionContext(),
            openHost: { [weak self] url in
                guard let ctx = self?.extensionContext else { return false }
                return await withCheckedContinuation { continuation in
                    ctx.open(url) { success in
                        continuation.resume(returning: success)
                    }
                }
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

    /// 收口(入队成功后;open 成功即刻 / 降级注记由用户「好」触发)。
    /// completeRequest 后扩展进程随时可被系统回收——入队先于本口完成
    /// (两步拷贝 + meta 翻 ready),队列状态落盘为准。
    private func finish() {
        extensionContext?.completeRequest(returningItems: nil, completionHandler: nil)
    }
}
