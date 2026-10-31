/// 分享扩展占位壳(T12 实装,spec §4.7/§5.4):T4 仅保 target 可构建——
/// 不跑引擎、不建 App Group、不做队列;NSExtensionPrincipalClass 指向本类。
///
/// 工程约束(spec §5.4):扩展不链 libios_bridge.a,内存限额+秒级寿命,
/// 设备缓存读主 App 写入的 App Group;两步拷贝(临时名+完成 rename)与
/// 「拷一半不静默丢弃」语义在 T12 落地。

import UIKit

final class ShareViewController: UIViewController {}
