import MTDSSKit
import XCTest

/// 扫码登录二维码内容解析单测(V3-1;纯函数面):
/// - 命中本家 URL 形态 → 取 `?t=` 为 qr token(与 login.qr_begin 的 qr_url
///   golden 同源往返);
/// - 非本家形态(他家 URL/纯文本/变体前缀)→ nil,调用方给「非 MTDSS
///   登录码」文案。
final class QrLoginCodeTests: XCTestCase {

    func testParsesGoldenQrUrlRoundTrip() throws {
        // 契约往返:login.qr_begin golden 的 qr_url 原文 → parse 出 token
        // = 同帧 token 字段(daemon 拼 URL 的单一真源口径)
        let frame = try JSONParser.parse(Goldens.text("login.qr_begin.resp.json"))
        guard case .object(let obj) = frame, case .object(let result)? = obj["result"] else {
            return XCTFail("login.qr_begin.resp.json 须为应答帧")
        }
        let begin = try QrBeginResult(.object(result))
        XCTAssertEqual(QrLoginCode.parse(begin.qrUrl), begin.token)
        XCTAssertEqual(begin.token, "qrt-contract-fixed")
    }

    func testParsesPlainTokenUrl() {
        XCTAssertEqual(QrLoginCode.parse("https://www.mtdss.top/qr?t=abc123"), "abc123")
    }

    func testToleratesSurroundingWhitespace() {
        XCTAssertEqual(
            QrLoginCode.parse("  https://www.mtdss.top/qr?t=tok-1 \n"),
            "tok-1",
            "扫描原文首尾空白容忍",
        )
    }

    func testTruncatesTokenAtAdditionalQueryParams() {
        XCTAssertEqual(
            QrLoginCode.parse("https://www.mtdss.top/qr?t=tok-1&x=2"),
            "tok-1",
            "t 后若拼了别的参数,只取本段",
        )
    }

    func testRejectsForeignUrlsAndText() {
        XCTAssertNil(QrLoginCode.parse("https://evil.example.com/qr?t=tok"), "他家 URL 不命中")
        XCTAssertNil(QrLoginCode.parse("http://www.mtdss.top/qr?t=tok"), "前缀降级 http 不命中(防变体)")
        XCTAssertNil(QrLoginCode.parse("https://WWW.MTDSS.TOP/qr?t=tok"), "前缀大小写敏感")
        XCTAssertNil(QrLoginCode.parse("随便扫的一段文本"), "纯文本不命中")
        XCTAssertNil(QrLoginCode.parse("https://www.mtdss.top/qr"), "无查询参数不命中")
        XCTAssertNil(QrLoginCode.parse("https://www.mtdss.top/qr?t="), "空 token 不命中")
        XCTAssertNil(QrLoginCode.parse("https://www.mtdss.top/qr?s=tok"), "非 t 参数不命中")
    }
}
