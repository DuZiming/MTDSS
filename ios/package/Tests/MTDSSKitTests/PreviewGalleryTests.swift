import XCTest
@testable import MTDSSKit

/// 图库切换几何宿主单测(V4-1 T25;安卓 ui/preview/PreviewGallery.kt 同构;
/// 钦定 mockup mobile-preview ‹› 切换 + 底部页点):图库集只收图片类文件 /
/// 同深度连续切片 = 兄弟集(展开子树不串层)/ 环绕索引纯函数。
final class PreviewGalleryTests: XCTestCase {

    private func entry(_ name: String, isDir: Bool = false) -> DirEntry {
        DirEntry(name: name, path: "/\(name)", size: 1, mtimeMs: 0, isDir: isDir)
    }

    func testImageSiblingsOnlyImages() {
        let files = [
            entry("a.jpg"),
            entry("notes.txt"),
            entry("sub", isDir: true),
            entry("b.PNG"),
            entry("c.mp4"),
            entry("d.heic"),
        ]
        let gallery = PreviewGallery.imageSiblings(files)
        XCTAssertEqual(gallery.map(\.path), ["/a.jpg", "/b.PNG", "/d.heic"])
    }

    func testWrapIndexWrapsBothWays() {
        XCTAssertEqual(PreviewGallery.wrapIndex(-1, 3), 2)
        XCTAssertEqual(PreviewGallery.wrapIndex(0, 3), 0)
        XCTAssertEqual(PreviewGallery.wrapIndex(3, 3), 0)
        XCTAssertEqual(PreviewGallery.wrapIndex(4, 3), 1)
        XCTAssertEqual(PreviewGallery.wrapIndex(0, 0), 0)
        XCTAssertEqual(PreviewGallery.wrapIndex(5, 1), 0)
    }

    func testSiblingFilesSlicesSameDepth() {
        // 展示序:a.jpg(0) b.txt(0) [展开 sub] x.png(1) y.mov(1) c.pdf(0)
        let rows = [
            BrowseRow(entry: entry("a.jpg"), depth: 0),
            BrowseRow(entry: entry("b.txt"), depth: 0),
            BrowseRow(entry: entry("sub", isDir: true), depth: 0),
            BrowseRow(entry: entry("x.png"), depth: 1),
            BrowseRow(entry: entry("y.mov"), depth: 1),
            BrowseRow(entry: entry("c.pdf"), depth: 0),
        ]
        // 点子树内文件(index 3,depth 1)= 兄弟集仅该目录的连续段
        XCTAssertEqual(
            PreviewGallery.siblingFiles(rows, index: 3).map(\.path),
            ["/x.png", "/y.mov"],
        )
        // 点根级文件(index 0,depth 0)= 整段根级行(子树行深度更深,连续包含)
        XCTAssertEqual(PreviewGallery.siblingFiles(rows, index: 0).count, 6)
    }

    func testSiblingFilesOutOfRangeFallsBackToSingle() {
        XCTAssertEqual(
            PreviewGallery.siblingFiles([BrowseRow(entry: entry("only.txt"), depth: 0)], index: 9)
                .map(\.path),
            ["/only.txt"],
        )
    }
}
