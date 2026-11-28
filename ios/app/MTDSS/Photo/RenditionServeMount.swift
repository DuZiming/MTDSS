/// App 侧转码桥装配(V4-1 T14;Ruling E 同款:宿主逻辑归 Kit 可测,绑定
/// 归 App):Kit ``RenditionResponder`` 消费 fs.rendition_open/cancel 事件,
/// 经 ``FdPassChannel``(fdpass rendition 通道 0x0C,单实例)应答:
/// - `/照片图库` 前缀 = 照片臂(批内唯一名重放 → PHPhotoLibraryService
///   openSeekable 资产文件 fd;iCloud 未卸载 = 拒词 icloud_not_local 透传,
///   引擎落既有回退);
/// - 其余 = 文件臂(授权根沙箱真实路径 DefaultSeekFiles 直开——引擎授权
///   根 std::fs 直读不经本桥,本臂为同构防御面);
/// - 转码 = ``IOSRenditionTranscoder``(AVAssetReader 解 + VTCompressionSession
///   硬编 AVC,CMAF/fMP4 分段),段文件落
///   ``EngineLayout/renditionServeDirectory``(`{home}/rendition-serve`)后
///   0x0C 逐段回告;引擎按需读已产段,cancel 即清,挂载清扫兜底残件。
///
/// **@MainActor**:构造面主线程(SeekServe/ThumbServe 同裁定);重活
/// (解/编/泵)在管线内部专用串行队列,await 不占主线程。
///
/// 红线(T2 携带):本文件不触发 set_recv_dir、不写 settings.json。

import AVFoundation
import Foundation
import MTDSSKit

enum RenditionServe {
    /// 生产应答器主环(App boot 时随事件订阅流挂起;事件流终结即退,
    /// PhotoServe/SeekServe/ThumbServe 同款——独立订阅流,各屏互不抢流)。
    @MainActor
    static func run(layout: EngineLayout, events: AsyncStream<IpcEvent>) async {
        let responder = RenditionResponder(
            service: PHPhotoLibraryService.shared,
            fdpass: FdPassChannel(socketPath: layout.fdpassSocketPath),
            renditionDir: layout.renditionServeDirectory,
            transcoder: IOSRenditionTranscoder(),
            durationProbe: IOSRenditionDurationProbe())
        await responder.run(events)
    }
}

/// 源总时长探针(r29 App 面):fd → F_GETPATH 沙箱路径 → AVURLAsset
/// .duration(元数据加载,毫秒级;失败/非有限 = 0 未知,落 EVENT 形)。
struct IOSRenditionDurationProbe: RenditionDurationProbing {
    func durationMs(fd: Int32, displayName: String) async -> UInt64 {
        var buf = [CChar](repeating: 0, count: Int(MAXPATHLEN))
        guard fcntl(fd, F_GETPATH, &buf) == 0 else { return 0 }
        let path = String(cString: buf)
        let asset = AVURLAsset(url: URL(fileURLWithPath: path))
        guard let duration = try? await asset.load(.duration),
              duration.isNumeric, duration.seconds > 0 else { return 0 }
        return UInt64((duration.seconds * 1000).rounded())
    }
}
