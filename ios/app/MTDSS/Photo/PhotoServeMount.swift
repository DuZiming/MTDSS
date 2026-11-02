/// App 侧照片桥装配(R7 Ruling P;Ruling E:宿主逻辑归 Kit 可测,绑定归
/// App):Kit ``PhotoResponder`` 消费 fs.photo_list / fs.photo_export 事件,
/// 经 ``FdPassChannel``(fdpass 查询通道,{home}/fds.sock)应答:
/// - 0x06 列表 = 相册(目录项)/ 照片(文件项),数据源 =
///   ``PHPhotoLibraryService``(权限门在应答方——非 authorized/limited
///   即拒答,引擎按桥不可用收口 → 虚拟根隐藏);
/// - 0x07 导出 = 原始字节(R5c 导出代码)落逐请求子目录
///   (``EngineLayout/photoServeDirectory``,App 私有隐藏目录),回绝对
///   路径;引擎读发后 unlink 文件,残目录由挂载清扫回收。
///
/// 红线(T2 携带):本文件不触发 set_recv_dir、不写 settings.json。

import Foundation
import MTDSSKit

enum PhotoServe {
    /// 生产应答器主环(App boot 时随事件订阅流挂起;事件流终结即退)。
    /// **@MainActor**:PHPhotoLibraryService 整体主线程隔离(R5c 实证
    /// PhotoKit fetch 非主线程实挂),应答循环挂主 actor 即全链主线程
    /// ——重活(writeData 导出)在服务实现内部自有队列,await 不占主线程。
    @MainActor
    static func run(layout: EngineLayout, events: AsyncStream<IpcEvent>) async {
        let responder = PhotoResponder(
            service: PHPhotoLibraryService.shared,
            fdpass: FdPassChannel(socketPath: layout.fdpassSocketPath),
            serveRoot: layout.photoServeDirectory,
        )
        await responder.run(events)
    }
}
