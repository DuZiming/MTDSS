/// ShareDisplayName 取名链单测(R5/J2):suggested → 落地名 → asset 原名 →
/// 兜底四道;纯扩展名下沉/扩展名补齐不丢不改/净化同锚。
import XCTest
@testable import MTDSSKit

final class ShareDisplayNameTests: XCTestCase {

    // MARK: 四道优先级

    func testSuggested在场直收() {
        XCTAssertEqual(
            ShareDisplayName.resolve(
                suggested: "IMG_1234.jpg", loadedName: nil, assetName: nil, fallbackExt: "jpg"),
            "IMG_1234.jpg")
    }

    func testSuggested空落地名收() {
        // 相册流 suggestedName 常空(R5 用户实机「分享文件.jpg」根因)→
        // 落地件真实名(file representation lastPathComponent)收
        XCTAssertEqual(
            ShareDisplayName.resolve(
                suggested: nil, loadedName: "IMG_20261108_093000.JPG", assetName: nil,
                fallbackExt: "jpg"),
            "IMG_20261108_093000.JPG")
    }

    func testSuggested落地俱缺Asset名收() {
        // 无落地 URL 形(仅 asset 手递):相册资源原名收
        XCTAssertEqual(
            ShareDisplayName.resolve(
                suggested: nil, loadedName: nil, assetName: "IMG_0001.heic", fallbackExt: "heic"),
            "IMG_0001.heic")
    }

    func test三道俱缺兜底分享文件() {
        XCTAssertEqual(
            ShareDisplayName.resolve(
                suggested: nil, loadedName: nil, assetName: nil, fallbackExt: "jpg"),
            "分享文件.jpg")
        XCTAssertEqual(
            ShareDisplayName.resolve(
                suggested: nil, loadedName: nil, assetName: nil, fallbackExt: nil),
            "分享文件")
    }

    // MARK: 纯扩展名下沉(suggested 特规)

    func testSuggested纯扩展名下沉() {
        // suggested「jpg」= 落地类型的纯扩展名,不算名 → 落地名收
        XCTAssertEqual(
            ShareDisplayName.resolve(
                suggested: "jpg", loadedName: "IMG_1.jpg", assetName: nil, fallbackExt: "jpg"),
            "IMG_1.jpg")
        XCTAssertEqual(
            ShareDisplayName.resolve(
                suggested: ".jpg", loadedName: "IMG_1.jpg", assetName: nil, fallbackExt: "jpg"),
            "IMG_1.jpg")
    }

    func testSuggested无点短词不冤枉() {
        // 无点短词(如「notes」)与落地扩展名不同字面 → 真名收
        XCTAssertEqual(
            ShareDisplayName.resolve(
                suggested: "notes", loadedName: "notes.txt", assetName: nil, fallbackExt: "txt"),
            "notes.txt")
        // 落地扩展名不可知 → 无从比对,恒不判纯扩展名
        XCTAssertEqual(
            ShareDisplayName.resolve(
                suggested: "notes", loadedName: nil, assetName: nil, fallbackExt: nil),
            "notes")
    }

    // MARK: 扩展名不丢不改

    func test无扩展按落地类型补() {
        XCTAssertEqual(
            ShareDisplayName.resolve(
                suggested: "README", loadedName: nil, assetName: nil, fallbackExt: "txt"),
            "README.txt")
    }

    func test异扩展名追加不剥不改() {
        // 旧 R10 语义逐字保:已有(异)扩展原样保 + 落地真扩展追加
        XCTAssertEqual(
            ShareDisplayName.resolve(
                suggested: "photo.png", loadedName: nil, assetName: nil, fallbackExt: "jpg"),
            "photo.png.jpg")
    }

    func test大小写扩展名视同相符() {
        XCTAssertEqual(
            ShareDisplayName.resolve(
                suggested: nil, loadedName: "IMG_1.JPG", assetName: nil, fallbackExt: "jpg"),
            "IMG_1.JPG")
    }

    // MARK: 净化与空名

    func test路径分量净化取末段() {
        XCTAssertEqual(
            ShareDisplayName.resolve(
                suggested: "a/b:图 片.jpg", loadedName: nil, assetName: nil, fallbackExt: "jpg"),
            "b_图 片.jpg")
    }

    func test空名逐道下沉() {
        // suggested 空串 → 落地空串 → asset 收
        XCTAssertEqual(
            ShareDisplayName.resolve(
                suggested: "", loadedName: "", assetName: "IMG_9.jpg", fallbackExt: "jpg"),
            "IMG_9.jpg")
    }

    // MARK: 链序钉死

    func test落地名在场压过Asset名() {
        // brief 钦定链序:② 落地名(哪怕系统临时名)先于 ③ asset 原名;
        // asset 只在无落地 URL 形生效
        XCTAssertEqual(
            ShareDisplayName.resolve(
                suggested: nil, loadedName: "0F9EC731-1B53.jpeg", assetName: "IMG_1.jpg",
                fallbackExt: "jpeg"),
            "0F9EC731-1B53.jpeg")
    }

    func testIsPureExtension判定面() {
        XCTAssertTrue(ShareDisplayName.isPureExtension("jpg", ext: "jpg"))
        XCTAssertTrue(ShareDisplayName.isPureExtension("JPG", ext: "jpg"))
        XCTAssertTrue(ShareDisplayName.isPureExtension(".png", ext: "png"))
        XCTAssertFalse(ShareDisplayName.isPureExtension("notes", ext: "txt"))
        XCTAssertFalse(ShareDisplayName.isPureExtension("a.jpg", ext: "jpg"))
        XCTAssertFalse(ShareDisplayName.isPureExtension("notes", ext: nil))
        XCTAssertFalse(ShareDisplayName.isPureExtension("notes", ext: ""))
    }
}
