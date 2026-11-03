/// 分享扩展主体(R10 直发形态;NSExtensionPrincipalClass):
/// - 壳 = UIViewController + UIHostingController(SharePanelView,SwiftUI
///   面板);**扩展链引擎**(R10,brief 架构 ①:libios_bridge.a device
///   slice + 扩展侧 `@_silgen_name` 绑定,见 EngineBindings.swift)——
///   点设备直发,表内进度条,完成即收口;
/// - 直发编排 = ``DirectSendEngine``(会话素材/批次帽/暂存/boot/发送/
///   事件泵;降级阶梯回落既有队列,主 App 下次打开消费——**两级拉起代码
///   退役**(open()/responder-chain,R6/R9 真机实证不拉起,spec §4.7 风险
///   注随 R10 关闭);
/// - completeRequest 收口:返回空 items(数据已随引擎发送/入队落盘为准)。
/// 全程 os_log(R6 起排障纪律;Console 按 subsystem top.mtdss.ios.share)。

import SwiftUI
import UIKit
import MTDSSKit
import os

private let shareVcLog = Logger(subsystem: "top.mtdss.ios.share", category: "vc")

final class ShareViewController: UIViewController {

    private let writer = ShareQueueWriter()
    private let engine = DirectSendEngine.make()

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .clear
        shareVcLog.info("extension viewDidLoad directSend=\(engine.available)")
        let panel = SharePanelView(
            writer: writer,
            context: extensionContext ?? NSExtensionContext(),
            engine: engine,
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

    /// 收口(引擎停机/批清弃在面板 finish() 内先行;此处仅请求收口)。
    /// completeRequest 后扩展进程随时可被系统回收——落盘状态为准。
    private func finish() {
        shareVcLog.info("completeRequest")
        extensionContext?.completeRequest(returningItems: nil, completionHandler: nil)
    }
}
