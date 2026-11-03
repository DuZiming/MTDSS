import XCTest
@testable import MTDSSKit

/// T8 布局派生单测(spec §5.2;先红后绿):
/// - **派生闭环**:容器 Documents → home = `{Documents 父}/Library/mtdss`
///   (App 私有,Files App 不可见);inbox/staging/socket 派生与桥
///   (crates/ios-bridge mtdss_ios_start)同款规则;
/// - **sun_path 预算**:真机最坏容器路径 + `/Library/mtdss` +
///   `/daemon.sock` < 104 字节(sockaddr_un.sun_path 含 NUL);
/// - **隐藏面**:暂存区不落发布根下(杀 App 中间态不可见的布局前提)。
final class EngineLayoutTests: XCTestCase {

    /// 真机最坏容器路径(/var/mobile/Containers/Data/Application/ + 36 字符
    /// UUID):文档化常数,派生后 socket 路径必须全数进 sun_path 预算。
    private static let worstCaseContainer =
        "/var/mobile/Containers/Data/Application/12345678-90AB-CDEF-1234-567890ABCDEF"

    private static let documents = worstCaseContainer + "/Documents"

    // MARK: 派生

    func testHomeDerivedFromContainerDocumentsIsPrivateLibraryPath() {
        let layout = EngineLayout(containerDocuments: Self.documents)
        XCTAssertEqual(
            layout.home,
            Self.worstCaseContainer + "/Library/mtdss",
            "home 必须派生为容器 Library/mtdss(App 私有)"
        )
    }

    func testInboxRoundTripsToContainerDocuments() {
        let layout = EngineLayout(containerDocuments: Self.documents)
        XCTAssertEqual(layout.inbox, Self.documents, "发布根/收件箱 = 容器 Documents")
        // 反向构造(直接给 home)同款 inbox:桥派生单一规则。
        XCTAssertEqual(EngineLayout(home: layout.home).inbox, Self.documents)
    }

    func testStagingIsHiddenMtdssUnderHome() {
        let layout = EngineLayout(containerDocuments: Self.documents)
        XCTAssertEqual(layout.staging, layout.home + "/.mtdss", "暂存 = {home}/.mtdss")
    }

    /// R8 终审 I-2:照片导出临时根 = home 直下 photo-serve——**在 `.mtdss`
    /// 暂存区命名空间之外**(对端顶层 rel "photo-serve" 的收件中间态落
    /// `<staging>/photo-serve`,serve 目录若在暂存区内会被挂载清扫在
    /// 中断窗摧毁;协议侧保留首段纵深防御),亦不落发布根(Files App
    /// 不可见)。
    func testPhotoServeDirectoryIsOutsideStagingNamespace() {
        let layout = EngineLayout(containerDocuments: Self.documents)
        XCTAssertEqual(layout.photoServeDirectory, layout.home + "/photo-serve")
        XCTAssertFalse(
            layout.photoServeDirectory.hasPrefix(layout.staging),
            "导出临时根不得落在暂存区命名空间内",
        )
        XCTAssertFalse(
            layout.photoServeDirectory.hasPrefix(layout.inbox),
            "导出临时根不得落在发布根(收件箱)下",
        )
    }

    func testSocketPathsLiveUnderHome() {
        let layout = EngineLayout(containerDocuments: Self.documents)
        XCTAssertEqual(layout.daemonSocketPath, layout.home + "/daemon.sock")
        XCTAssertEqual(layout.fdpassSocketPath, layout.home + "/fds.sock")
    }

    // MARK: sun_path 预算

    func testWorstCaseDeviceSocketPathsFitSunPath() {
        let layout = EngineLayout(containerDocuments: Self.documents)
        XCTAssertTrue(
            layout.socketPathsFitSunPath,
            "daemon.sock(\(layout.daemonSocketPath.utf8.count)B)与 fds.sock"
                + "(\(layout.fdpassSocketPath.utf8.count)B)必须都 < \(EngineLayout.sunPathLimit)"
        )
        let daemonBytes = layout.daemonSocketPath.utf8.count
        XCTAssertLessThan(daemonBytes, EngineLayout.sunPathLimit, "最坏容器 + {home}/daemon.sock")
        XCTAssertLessThan(layout.fdpassSocketPath.utf8.count, EngineLayout.sunPathLimit)
        // 预算账本(报告用):102 字节 = 40(容器前缀)+ 36(UUID)+ 14(/Library/mtdss)+ 12(/daemon.sock)。
        XCTAssertEqual(daemonBytes, 102)
    }

    func testOversizedHomeIsRejectedByBudget() {
        // 模拟器容器路径(≈170B,实测超限)必须被预算判负——sim 冒烟必须
        // 用短 home env 覆盖(T4/T7 已知面)。
        let long = "/Users/someone/Library/Developer/CoreSimulator/Devices/"
            + "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE/data/Containers/Data/Application/"
            + "12345678-90AB-CDEF-1234-567890ABCDEF/Documents"
        let layout = EngineLayout(containerDocuments: long)
        XCTAssertFalse(layout.socketPathsFitSunPath, "模拟器容器路径必须超预算")
    }

    // MARK: 隐藏面

    func testStagingIsNotUnderInbox() {
        let layout = EngineLayout(containerDocuments: Self.documents)
        XCTAssertTrue(layout.stagingHiddenFromInbox, "暂存区不得落在发布根下")
    }
}
