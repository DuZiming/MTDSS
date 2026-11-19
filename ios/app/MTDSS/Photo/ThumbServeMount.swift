/// App 侧缩略图桥装配(V4-1 T11;Ruling E 同款:宿主逻辑归 Kit 可测,
/// 绑定归 App):Kit ``ThumbResponder`` 消费 fs.request_thumb 事件,经
/// ``FdPassChannel``(fdpass thumb 通道 0x0B,单实例)应答:
/// - `/照片图库` 前缀 = 照片臂(批内唯一名重放 → PHPhotoLibraryService
///   openSeekable 资产文件 fd;iCloud 未卸载 = 拒词 icloud_not_local 透
///   传,引擎落既有回退);
/// - 其余 = 文件臂(授权根沙箱真实路径 DefaultSeekFiles 直开——引擎授权
///   根 std::fs 直读不经本桥,本臂为同构防御面)。
/// - JPEG 生成 = ``IOSThumbEncoder``(AVAssetImageGenerator 视频抽帧 /
///   UIImage 系统解码含 HEIC 降采样 → JPEG),落
///   ``EngineLayout/thumbServeDirectory``(`{home}/thumb-serve`)后 0x0B
///   回路径;引擎读毕 unlink,挂载清扫兜底残件。
///
/// **@MainActor**:照片臂全链主线程(SeekServe 同裁定——PhotoKit fetch
/// 非主线程实挂);重活(解码/压缩)在编码器实现内部 Task.detached,
/// await 不占主线程。
///
/// 红线(T2 携带):本文件不触发 set_recv_dir、不写 settings.json。

import Foundation
import MTDSSKit

enum ThumbServe {
    /// 生产应答器主环(App boot 时随事件订阅流挂起;事件流终结即退,
    /// PhotoServe/SeekServe 同款——独立订阅流,各屏互不抢流)。
    @MainActor
    static func run(layout: EngineLayout, events: AsyncStream<IpcEvent>) async {
        let responder = ThumbResponder(
            service: PHPhotoLibraryService.shared,
            fdpass: FdPassChannel(socketPath: layout.fdpassSocketPath),
            thumbDir: layout.thumbServeDirectory,
            encoder: IOSThumbEncoder(),
        )
        await responder.run(events)
    }
}
