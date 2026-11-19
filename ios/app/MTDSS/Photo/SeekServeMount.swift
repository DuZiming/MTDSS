/// App 侧 seek 源桥装配(V4-1 T10;Ruling E 同款:宿主逻辑归 Kit 可测,
/// 绑定归 App):Kit ``SeekResponder`` 消费 fs.request_seek 事件,经
/// ``FdPassChannel``(fdpass seek 通道 0x0D,SCM_RIGHTS 随帧)应答:
/// - `/照片图库` 前缀 = 照片臂(批内唯一名重放 → PHPhotoLibraryService
///   openSeekable 资产文件 URL 直读;iCloud 未卸载 = 拒开词面
///   icloud_not_local,引擎落既有导出回退);
/// - 其余 = 文件臂(授权根沙箱真实路径 FileHandle 直开——iOS 无 SAF/
///   bridge 根,引擎授权根 std::fs 直读不经本桥,本臂为同构防御面)。
///
/// **@MainActor**:PHPhotoLibraryService 整体主线程隔离(R5c 实证 PhotoKit
/// fetch 非主线程实挂),应答循环挂主 actor 即照片臂全链主线程——重活
/// (requestImageData 数据拉取)在服务实现内部自有队列,await 不占主线程;
/// 文件臂 FileHandle 直开微秒级,主线程无害。
///
/// 红线(T2 携带):本文件不触发 set_recv_dir、不写 settings.json。

import Foundation
import MTDSSKit

enum SeekServe {
    /// 生产应答器主环(App boot 时随事件订阅流挂起;事件流终结即退,
    /// PhotoServe 同款——独立订阅流,各屏互不抢流)。
    @MainActor
    static func run(layout: EngineLayout, events: AsyncStream<IpcEvent>) async {
        let responder = SeekResponder(
            service: PHPhotoLibraryService.shared,
            fdpass: FdPassChannel(socketPath: layout.fdpassSocketPath),
        )
        await responder.run(events)
    }
}
