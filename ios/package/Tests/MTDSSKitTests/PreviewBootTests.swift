import XCTest
@testable import MTDSSKit

/// WebView boot 注入单点宿主单测(V4-1 T25):注入形状与桌面 preview_popout
/// `window.__MTDSS_PREVIEW_BOOT__ = {req, handle}`(src-tauri popout.rs +
/// previewer-main.ts 消费面)/ 安卓 ui/preview/PreviewBoot.kt 逐字对齐——
/// req = PreviewRequest JSON {kind, urls{raw}, meta{name,size}},raw 恒携
/// `?t=` 查询令牌(T20 面)。三侧漂移先红本套。
final class PreviewBootTests: XCTestCase {

    private let req = PreviewBoot.requestJson(
        kind: "pdf",
        rawUrl: "http://127.0.0.1:8000/f/h1/raw?t=tk",
        name: "说明书.pdf",
        size: 123,
    )

    func testConstantsAnchor() {
        XCTAssertEqual(PreviewBoot.bootGlobal, "__MTDSS_PREVIEW_BOOT__")
        XCTAssertEqual(PreviewBoot.event, "mtdss-boot")
    }

    func testRequestJsonShape() {
        XCTAssertEqual(
            req,
            "{\"kind\":\"pdf\"," +
                "\"urls\":{\"raw\":\"http://127.0.0.1:8000/f/h1/raw?t=tk\"}," +
                "\"meta\":{\"name\":\"说明书.pdf\",\"size\":123}}",
        )
        XCTAssertTrue(req.contains("raw?t="), "raw 恒携 ?t= 查询令牌")
    }

    func testRequestJsonAutoPlayFlag() {
        // r49 #2:旗缺 = JSON 不携(现状逐字节);旗在场 = req 根 autoPlay:true
        let off = PreviewBoot.requestJson(
            kind: "video", rawUrl: "http://127.0.0.1:8000/f/h1/raw?t=tk",
            name: "影片.mp4", size: 1,
        )
        XCTAssertFalse(off.contains("autoPlay"), "旗缺不携键(向后兼容)")
        let on = PreviewBoot.requestJson(
            kind: "video", rawUrl: "http://127.0.0.1:8000/f/h1/raw?t=tk",
            name: "影片.mp4", size: 1, autoPlay: true,
        )
        XCTAssertEqual(
            on,
            "{\"kind\":\"video\"," +
                "\"urls\":{\"raw\":\"http://127.0.0.1:8000/f/h1/raw?t=tk\"}," +
                "\"meta\":{\"name\":\"影片.mp4\",\"size\":1},\"autoPlay\":true}",
        )
    }

    func testRequestJsonEscapesQuotesAndBackslashes() {
        let escaped = PreviewBoot.requestJson(
            kind: "text",
            rawUrl: "http://127.0.0.1:8000/f/h1/raw?t=tk",
            name: "a\"b\\c",
            size: 1,
        )
        XCTAssertEqual(
            escaped,
            "{\"kind\":\"text\"," +
                "\"urls\":{\"raw\":\"http://127.0.0.1:8000/f/h1/raw?t=tk\"}," +
                "\"meta\":{\"name\":\"a\\\"b\\\\c\",\"size\":1}}",
        )
    }

    func testBootJsonKeyShape() {
        let boot = PreviewBoot.bootJson(reqJson: req, handle: "h1")
        XCTAssertEqual(boot, "{\"req\":\(req),\"handle\":\"h1\"}")
    }

    func testInjectionJsAssignsAndDispatches() {
        let js = PreviewBoot.injectionJs(PreviewBoot.bootJson(reqJson: req, handle: "h1"))
        XCTAssertTrue(
            js.hasPrefix("window.__MTDSS_PREVIEW_BOOT__ = {\"req\":"),
            "赋值 window.__MTDSS_PREVIEW_BOOT__",
        )
        XCTAssertTrue(js.contains("\"handle\":\"h1\""), "句柄键入载荷")
        XCTAssertTrue(
            js.hasSuffix(";document.dispatchEvent(new Event(\"mtdss-boot\"));"),
            "派发 mtdss-boot 事件(宿主页模块脚本消费)",
        )
    }

    func testBootJsonEscapesHandle() {
        let boot = PreviewBoot.bootJson(reqJson: req, handle: "h\"1")
        XCTAssertTrue(boot.contains("\"handle\":\"h\\\"1\""))
    }
}
