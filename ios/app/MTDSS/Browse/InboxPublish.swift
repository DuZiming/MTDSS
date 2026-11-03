/// App 侧收件箱发布装配(T8;Ruling E:宿主逻辑归 Kit 可测,绑定归 App):
/// - Kit ``InboxPublisher`` 消费 fs.publish / fs.query_dest 事件,经
///   ``FdPassChannel``(fdpass 查询通道,{home}/fds.sock)应答:
///   发布 = move 暂存终件进容器 Documents 本名(撞名 ` (n)` 顺延),
///   在场查询 = 收件箱真实行;
/// - 生产三件套:EngineLayout(home 派生)/ FdPassChannel / Default
///   InboxFilesystem——见 ``ProductionEngine/layout()``。
///
/// 红线(T2 携带):本文件不触发 set_recv_dir、不写 settings.json。

import Foundation
import MTDSSKit

enum InboxPublish {
    /// 生产发布器主环(App boot 时随事件订阅流挂起;事件流终结即退)。
    static func run(layout: EngineLayout, events: AsyncStream<IpcEvent>) async {
        let publisher = InboxPublisher(
            layout: layout,
            fdpass: FdPassChannel(socketPath: layout.fdpassSocketPath),
            files: DefaultInboxFilesystem(),
        )
        await publisher.run(events)
    }
}
