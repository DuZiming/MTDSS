import XCTest
@testable import MTDSSKit

/// 预览降级面文案宿主单测(V4-1 T25;安卓 ui/preview/PreviewErrors.kt 同构):
/// 源离线 mockup 确认点⑤ 词面「源设备离线,无法预览」;T10 icloud_not_local
/// 词透出 =「原件在 iCloud,需先下载」(远端 iPhone 源 + 自机照片库未卸载
/// 项同词,不静默全量拉);daemon RPC 错误消息(ctrl 帧 message 直透)→
/// 用户面映射纯函数。
final class PreviewErrorsTests: XCTestCase {

    func testIcloudNotLocalWordSurfaces() {
        XCTAssertEqual(
            PreviewErrors.userFace("远端: icloud_not_local 原件未下载到本机"),
            "原件在 iCloud,需先下载",
            "icloud_not_local 稳定词在消息中即透出 iCloud 面",
        )
        XCTAssertEqual(PreviewErrors.userFace("icloud_not_local"), "原件在 iCloud,需先下载")
    }

    func testOfflineFamilyNormalized() {
        // 远端设备离线(ctrl PeerOffline 词)
        XCTAssertEqual(PreviewErrors.userFace("设备离线: MIX 3"), "源设备离线,无法预览")
        // 本机源打开失败(api.rs source_offline 词)
        XCTAssertEqual(PreviewErrors.userFace("源打开失败: Permission denied"), "源设备离线,无法预览")
        // 稳定词形(peer_offline)
        XCTAssertEqual(PreviewErrors.userFace("peer_offline: 连接失败"), "源设备离线,无法预览")
    }

    func testOtherMessagesPassThrough() {
        XCTAssertEqual(PreviewErrors.userFace("文件已变更,请重试"), "文件已变更,请重试")
    }

    func testEmptyFallsToGeneric() {
        XCTAssertEqual(PreviewErrors.userFace(nil), "无法预览该文件")
        XCTAssertEqual(PreviewErrors.userFace(""), "无法预览该文件")
        XCTAssertEqual(PreviewErrors.userFace("   "), "无法预览该文件")
    }

    func testFaceConstantsAnchor() {
        XCTAssertEqual(PreviewErrors.offlineFace, "源设备离线,无法预览")
        XCTAssertEqual(PreviewErrors.icloudFace, "原件在 iCloud,需先下载")
        XCTAssertEqual(PreviewErrors.genericFace, "无法预览该文件")
    }
}
