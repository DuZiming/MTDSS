import XCTest
@testable import MTDSSKit

/// 预览路由纯函数宿主单测(V4-1 T25;桌面 ui/src/lib/preview.ts KIND_BY_EXT
/// / 安卓 ui/preview/PreviewRoute.kt 词表同源逐字对齐——三侧漂移先红本套):
/// 容器四分路由 / HEIC 面 / 回环 URL 形 / doc kind 词面 / open-as MIME 表 /
/// iOS WKWebView 载体面(自管 scheme + 反解映射 + 导航守卫)。
final class PreviewRouteTests: XCTestCase {

    // MARK: 容器四分(词表矩阵大小写同真)

    func testImageExtsRouteToImage() {
        for ext in ["jpg", "jpeg", "png", "gif", "bmp", "webp", "heic", "svg", "tif", "tiff", "ico", "avif"] {
            XCTAssertEqual(
                PreviewRoute.containerOf(name: "photo.\(ext)"),
                .image,
                "ext=[\(ext)] → image",
            )
        }
        XCTAssertEqual(PreviewRoute.containerOf(name: "照片.JPG"), .image)
        XCTAssertEqual(PreviewRoute.containerOf(name: "photo.HeIc"), .image)
    }

    func testVideoExtsRouteToVideo() {
        for ext in ["mp4", "mkv", "mov", "avi", "webm", "m4v", "wmv", "flv", "mpg", "mpeg", "3gp"] {
            XCTAssertEqual(
                PreviewRoute.containerOf(name: "clip.\(ext)"),
                .video,
                "ext=[\(ext)] → video",
            )
        }
        XCTAssertEqual(PreviewRoute.containerOf(name: "录像.MP4"), .video)
    }

    func testAudioExtsRouteToAudio() {
        for ext in ["mp3", "flac", "wav", "aac", "ogg", "m4a", "wma", "opus"] {
            XCTAssertEqual(
                PreviewRoute.containerOf(name: "song.\(ext)"),
                .audio,
                "ext=[\(ext)] → audio",
            )
        }
    }

    func testDocExtsRouteToDoc() {
        for name in [
            "a.pdf", "a.doc", "a.docx", "a.xls", "a.xlsx", "a.csv", "a.ppt", "a.pptx",
            "a.md", "a.txt", "a.log",
            "a.rs", "a.ts", "a.tsx", "a.js", "a.mjs", "a.cjs", "a.jsx", "a.py", "a.go",
            "a.java", "a.kt", "a.kts", "a.swift", "a.c", "a.h", "a.cpp", "a.cc", "a.cxx",
            "a.hpp", "a.cs", "a.rb", "a.php", "a.sh", "a.bash", "a.zsh", "a.ps1", "a.bat",
            "a.json", "a.jsonc", "a.yml", "a.yaml", "a.toml", "a.ini", "a.cfg", "a.conf",
            "a.xml", "a.html", "a.css", "a.sql",
            "README.MD", "main.TS",
        ] {
            XCTAssertEqual(
                PreviewRoute.containerOf(name: name),
                .doc,
                "name=[\(name)] → doc",
            )
        }
    }

    func testUnknownAndDotfilesFallToUnsupported() {
        for name in ["archive.zip", "无扩展名", ".gitignore", "trailing.", "bin"] {
            XCTAssertEqual(
                PreviewRoute.containerOf(name: name),
                .unsupported,
                "name=[\(name)] → unsupported",
            )
        }
    }

    // MARK: HEIC 面(jpeg 转码端点仅对 heic 装配)

    func testIsHeic() {
        XCTAssertTrue(PreviewRoute.isHeic(name: "a.heic"))
        XCTAssertTrue(PreviewRoute.isHeic(name: "A.HEIC"))
        XCTAssertFalse(PreviewRoute.isHeic(name: "a.jpg"))
        XCTAssertFalse(PreviewRoute.isHeic(name: "heic"))
        XCTAssertFalse(PreviewRoute.isHeic(name: "a.heic.jpg"))
    }

    func testImageFacesHeicJpegFirstElseRawOnly() {
        XCTAssertEqual(PreviewRoute.imageFaces(name: "a.heic"), ["jpeg", "raw"])
        XCTAssertEqual(PreviewRoute.imageFaces(name: "a.jpg"), ["raw"])
        XCTAssertEqual(PreviewRoute.imageFaces(name: "a.png"), ["raw"])
    }

    // MARK: 回环 URL 形逐字(与桌面 previewLoopbackUrl 同形)

    func testLoopbackUrlShapes() {
        XCTAssertEqual(
            PreviewRoute.rawUrl(port: 8123, handle: "h1", token: "tok"),
            "http://127.0.0.1:8123/f/h1/raw?t=tok",
        )
        XCTAssertEqual(
            PreviewRoute.jpegUrl(port: 8123, handle: "h1", token: "tok"),
            "http://127.0.0.1:8123/f/h1/jpeg?t=tok",
        )
        XCTAssertEqual(
            BrowseThumbs.thumbUrl(port: 8123, handle: "h1", token: "tok"),
            "http://127.0.0.1:8123/f/h1/thumb?w=256&t=tok",
        )
    }

    // MARK: doc kind 词面(preview.ts KIND_BY_EXT office/md/text 段同源)

    func testDocKindOf() {
        XCTAssertEqual(PreviewRoute.docKindOf(name: "a.pdf"), "pdf")
        XCTAssertEqual(PreviewRoute.docKindOf(name: "a.doc"), "docx")
        XCTAssertEqual(PreviewRoute.docKindOf(name: "a.docx"), "docx")
        XCTAssertEqual(PreviewRoute.docKindOf(name: "a.xls"), "xlsx")
        XCTAssertEqual(PreviewRoute.docKindOf(name: "a.xlsx"), "xlsx")
        XCTAssertEqual(PreviewRoute.docKindOf(name: "a.csv"), "xlsx")
        XCTAssertEqual(PreviewRoute.docKindOf(name: "a.ppt"), "pptx")
        XCTAssertEqual(PreviewRoute.docKindOf(name: "a.pptx"), "pptx")
        XCTAssertEqual(PreviewRoute.docKindOf(name: "a.md"), "md")
        XCTAssertEqual(PreviewRoute.docKindOf(name: "a.txt"), "text")
        XCTAssertEqual(PreviewRoute.docKindOf(name: "a.rs"), "text")
        XCTAssertEqual(PreviewRoute.docKindOf(name: "a.json"), "text")
        XCTAssertNil(PreviewRoute.docKindOf(name: "a.jpg"), "非 doc 容器无 kind")
        XCTAssertNil(PreviewRoute.docKindOf(name: "a.mp4"), "非 doc 容器无 kind")
        XCTAssertNil(PreviewRoute.docKindOf(name: "a.zip"), "未知扩展无 kind")
    }

    // MARK: open-as MIME 表(未知落 octet-stream)

    func testMimeOf() {
        XCTAssertEqual(PreviewRoute.mimeOf(name: "a.jpg"), "image/jpeg")
        XCTAssertEqual(PreviewRoute.mimeOf(name: "a.heic"), "image/heic")
        XCTAssertEqual(PreviewRoute.mimeOf(name: "a.png"), "image/png")
        XCTAssertEqual(PreviewRoute.mimeOf(name: "a.mp4"), "video/mp4")
        XCTAssertEqual(PreviewRoute.mimeOf(name: "a.mov"), "video/quicktime")
        XCTAssertEqual(PreviewRoute.mimeOf(name: "a.mkv"), "video/x-matroska")
        XCTAssertEqual(PreviewRoute.mimeOf(name: "a.mp3"), "audio/mpeg")
        XCTAssertEqual(PreviewRoute.mimeOf(name: "a.flac"), "audio/flac")
        XCTAssertEqual(PreviewRoute.mimeOf(name: "a.m4a"), "audio/mp4")
        XCTAssertEqual(PreviewRoute.mimeOf(name: "a.pdf"), "application/pdf")
        XCTAssertEqual(PreviewRoute.mimeOf(name: "a.txt"), "text/plain")
        XCTAssertEqual(PreviewRoute.mimeOf(name: "a.md"), "text/plain")
        XCTAssertEqual(
            PreviewRoute.mimeOf(name: "a.docx"),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        )
        XCTAssertEqual(
            PreviewRoute.mimeOf(name: "a.xlsx"),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        )
        XCTAssertEqual(
            PreviewRoute.mimeOf(name: "a.pptx"),
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        )
        XCTAssertEqual(PreviewRoute.mimeOf(name: "a.doc"), "application/msword")
        XCTAssertEqual(PreviewRoute.mimeOf(name: "a.csv"), "text/csv")
        XCTAssertEqual(
            PreviewRoute.mimeOf(name: "a.zip"),
            "application/octet-stream",
            "未知扩展 → octet-stream",
        )
        XCTAssertEqual(
            PreviewRoute.mimeOf(name: "MIME"),
            "application/octet-stream",
            "无扩展名 → octet-stream",
        )
    }

    // MARK: WKWebView 载体面(T25;iOS 自管 scheme 同源)

    func testProxySchemeConstants() {
        XCTAssertEqual(PreviewRoute.proxyScheme, "mtdsspreview")
        XCTAssertEqual(PreviewRoute.hostPageUrl, "mtdsspreview://localhost/preview.html")
    }

    func testProxyRawUrlShape() {
        XCTAssertEqual(
            PreviewRoute.proxyRawUrl(port: 8123, handle: "h1", token: "tok"),
            "mtdsspreview://127.0.0.1:8123/f/h1/raw?t=tok",
            "boot raw 走自管 scheme,反解面见 loopbackUrl",
        )
    }

    func testLoopbackUrlFromProxy() {
        let proxy = URL(string: "mtdsspreview://127.0.0.1:8123/f/h1/raw?t=tok")
        XCTAssertEqual(
            PreviewRoute.loopbackUrl(from: proxy)?.absoluteString,
            "http://127.0.0.1:8123/f/h1/raw?t=tok",
        )
    }

    func testLoopbackUrlRejectsForeignShapes() {
        // 非 127.0.0.1 host(代理面收紧,防 handler 被当任意出口)
        XCTAssertNil(PreviewRoute.loopbackUrl(from: URL(string: "mtdsspreview://evil.example:1/f/h/raw?t=x")))
        // 无 port
        XCTAssertNil(PreviewRoute.loopbackUrl(from: URL(string: "mtdsspreview://127.0.0.1/f/h/raw?t=x")))
        // 非资产/代理路径(handler 资产臂之外的未知路径不代理)
        XCTAssertNil(PreviewRoute.loopbackUrl(from: URL(string: "mtdsspreview://127.0.0.1:1/other/x")))
        // 非 http 面(资产臂)
        XCTAssertNil(PreviewRoute.loopbackUrl(from: URL(string: "mtdsspreview://127.0.0.1:8123/preview.html")))
        // 其他 scheme 与空
        XCTAssertNil(PreviewRoute.loopbackUrl(from: URL(string: "https://127.0.0.1:1/f/h/raw")))
        XCTAssertNil(PreviewRoute.loopbackUrl(from: nil))
    }

    // MARK: 代理臂出站 URLRequest(fix round 1 Important #1:Range 头转发)

    func testProxyRequestForwardsRangeHeader() {
        var incoming = URLRequest(url: URL(string: "mtdsspreview://127.0.0.1:8123/f/h1/raw?t=tok")!)
        incoming.setValue("bytes=0-1023", forHTTPHeaderField: "Range")
        let out = try? XCTUnwrap(PreviewRoute.proxyRequest(for: incoming))
        XCTAssertEqual(out?.url?.absoluteString, "http://127.0.0.1:8123/f/h1/raw?t=tok")
        XCTAssertEqual(
            out?.value(forHTTPHeaderField: "Range"),
            "bytes=0-1023",
            "Accept-Ranges 广告即须兑现:出站携 Range,daemon 段取 206",
        )
    }

    func testProxyRequestWithoutRangeOmitsHeader() {
        let incoming = URLRequest(url: URL(string: "mtdsspreview://127.0.0.1:8123/f/h1/raw?t=tok")!)
        let out = try? XCTUnwrap(PreviewRoute.proxyRequest(for: incoming))
        XCTAssertNotNil(out)
        XCTAssertNil(out?.value(forHTTPHeaderField: "Range"), "无 Range 请求不出站造头")
    }

    func testProxyRequestRejectsForeignShapes() {
        // 异形 URL 不出站(收紧面与 loopbackUrl 同源)
        var foreign = URLRequest(url: URL(string: "https://example.com/f/h/raw")!)
        foreign.setValue("bytes=0-1", forHTTPHeaderField: "Range")
        XCTAssertNil(PreviewRoute.proxyRequest(for: foreign))
        XCTAssertNil(PreviewRoute.proxyRequest(for: nil))
    }

    // MARK: WebView 导航守卫(T23 教训 iOS 面;特权面永不导外部内容)

    func testLinkNavigable() {
        // 自管资产面放行(宿主页 + 任意同 scheme 面)
        XCTAssertTrue(PreviewRoute.linkNavigable("mtdsspreview://localhost/preview.html"))
        // file 面放行(安卓载体同位;iOS 守卫同放,语义面单源)
        XCTAssertTrue(PreviewRoute.linkNavigable("file:///bundle/previewer/preview.html"))
        // 外链/危险 scheme 全拒
        for url in [
            "https://example.com/a",
            "http://127.0.0.1:8123/f/x/raw?t=t",
            "intent://foo",
            "javascript:void(0)",
            "data:text/html,x",
            "about:blank",
            "",
        ] {
            XCTAssertFalse(PreviewRoute.linkNavigable(url), "url=[\(url)] 拒于特权面")
        }
    }
}
