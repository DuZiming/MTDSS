import XCTest
import MTDSSKit

/// 浏览缩略图纯函数面单测(V4-1 T24;镜像 android BrowseThumbsTest 16 断言
/// 面):缩略图资格矩阵 / 时长角标格式化(桌面 ui/src/lib/duration.ts
/// fmtDuration 逐字镜像)/ 回环 URL 组装含 ?t= 令牌 / 常量锚。
final class BrowseThumbsTests: XCTestCase {

    // MARK: 常量锚(brief 钉死值)

    func testConstantAnchors() {
        XCTAssertEqual(BrowseThumbs.thumbWidth, 256, "缩略图请求宽 = daemon THUMB_DEFAULT_WIDTH 同锚,两处不得漂移")
        XCTAssertEqual(BrowseThumbs.gridColumns, 4, "平铺列数 = mockup 393pt 屏宽一行 4 枚磁贴")
    }

    // MARK: 缩略图资格矩阵(图片/视频文件;目录/无扩展名/点文件/未知 = 图标占位)

    func testThumbEligibleMatrix() {
        let images = ["jpg", "jpeg", "png", "gif", "bmp", "webp", "heic", "svg", "tif", "tiff", "ico", "avif"]
        for ext in images {
            XCTAssertTrue(BrowseThumbs.thumbEligible(name: "IMG.\(ext)", isDir: false), ext)
            XCTAssertTrue(BrowseThumbs.thumbEligible(name: "IMG.\(ext.uppercased())", isDir: false), "\(ext) 大写同真")
        }
        let videos = ["mp4", "mkv", "mov", "avi", "webm", "m4v", "wmv", "flv", "mpg", "mpeg", "3gp"]
        for ext in videos {
            XCTAssertTrue(BrowseThumbs.thumbEligible(name: "VID_0032.\(ext)", isDir: false), ext)
        }
        // r30:PDF 出缩略图(App 侧 QL 后端;平台后端无 PDF 面的移动对齐)
        XCTAssertTrue(BrowseThumbs.thumbEligible(name: "doc.pdf", isDir: false), "pdf 出缩略图(r30)")
        XCTAssertTrue(BrowseThumbs.thumbEligible(name: "DOC.PDF", isDir: false), "pdf 大写同真")
        // 目录恒假(带图片扩展名也不出缩略图)
        XCTAssertFalse(BrowseThumbs.thumbEligible(name: "相册.jpg", isDir: true))
        // 无扩展名 / 点文件 / 尾点 / 未知扩展名 → 图标占位
        XCTAssertFalse(BrowseThumbs.thumbEligible(name: "noext", isDir: false))
        XCTAssertFalse(BrowseThumbs.thumbEligible(name: ".bashrc", isDir: false), "点在首位 = 点文件")
        XCTAssertFalse(BrowseThumbs.thumbEligible(name: "name.", isDir: false), "点在末尾 = 无扩展名")
        XCTAssertFalse(BrowseThumbs.thumbEligible(name: "sheet.xlsx", isDir: false), "office 不出(留后续)")
    }

    // MARK: 视频名判定(时长角标只落视频,图片有 secs 也不显)

    func testIsVideoName() {
        XCTAssertTrue(BrowseThumbs.isVideoName("IMG_2041.mp4"))
        XCTAssertTrue(BrowseThumbs.isVideoName("clip.MOV"), "大写扩展名同真")
        XCTAssertFalse(BrowseThumbs.isVideoName("横版照片.jpg"), "图片名不显角标")
        XCTAssertFalse(BrowseThumbs.isVideoName("noext"))
    }

    // MARK: 时长角标(桌面 fmtDuration 逐字镜像:m:ss 分位不补零 / h:mm:ss)

    func testDurationBadgeFormats() {
        // mockup 值:91.5s 视频行 → 1:31
        XCTAssertEqual(BrowseThumbs.durationBadge(91.5), "1:31", "小数向下取整")
        XCTAssertEqual(BrowseThumbs.durationBadge(192), "3:12")
        XCTAssertEqual(BrowseThumbs.durationBadge(47), "0:47", "m:ss 分位不补零(m=0)")
        XCTAssertEqual(BrowseThumbs.durationBadge(60), "1:00", "秒位恒补零")
        XCTAssertEqual(BrowseThumbs.durationBadge(59.9), "0:59", "floor 不进位")
        // 满 1 小时:h:mm:ss(时/分位补零)
        XCTAssertEqual(BrowseThumbs.durationBadge(3600), "1:00:00")
        XCTAssertEqual(BrowseThumbs.durationBadge(3723), "1:02:03")
        XCTAssertEqual(BrowseThumbs.durationBadge(7322), "2:02:02")
    }

    func testDurationBadgeUnknownIsEmptiness() {
        // 有则显无则不显:未知/非正/非数一律空串,角标不落垃圾
        XCTAssertEqual(BrowseThumbs.durationBadge(nil), "", "未知(nil)→ 空串")
        XCTAssertEqual(BrowseThumbs.durationBadge(0), "", "零 = 未在册时长")
        XCTAssertEqual(BrowseThumbs.durationBadge(-3.2), "", "负数不落角标")
        XCTAssertEqual(BrowseThumbs.durationBadge(.infinity), "", "非有限数不落角标")
        XCTAssertEqual(BrowseThumbs.durationBadge(.nan), "", "NaN 不落角标")
    }

    func testDurationBadgeForOnlyVideos() {
        XCTAssertEqual(BrowseThumbs.durationBadgeFor(name: "IMG_2041.mp4", secs: 91.5), "1:31")
        XCTAssertEqual(BrowseThumbs.durationBadgeFor(name: "IMG_2041.mp4", secs: nil), "")
        XCTAssertEqual(BrowseThumbs.durationBadgeFor(name: "横版照片.jpg", secs: 91.5), "", "图片有 secs 也不显(mockup 同)")
        XCTAssertEqual(BrowseThumbs.durationBadgeFor(name: "文档.pdf", secs: 10), "")
    }

    // MARK: 回环 URL 组装(?t= 查询令牌与 Bearer 双缝等价,T20 面)

    func testThumbUrlContainsPortHandleWidthAndToken() {
        let url = BrowseThumbs.thumbUrl(port: 39801, handle: "prev-fixed-1", token: "tok-abc")
        XCTAssertEqual(url, "http://127.0.0.1:39801/f/prev-fixed-1/thumb?w=256&t=tok-abc")
        // 令牌逐字透传(URL 不转义不破坏)
        let special = BrowseThumbs.thumbUrl(port: 1, handle: "h", token: "aB3-_~")
        XCTAssertTrue(special.contains("t=aB3-_~"), special)
        // 自定义宽
        XCTAssertEqual(
            BrowseThumbs.thumbUrl(port: 2, handle: "h", token: "t", w: 512),
            "http://127.0.0.1:2/f/h/thumb?w=512&t=t")
    }
}
