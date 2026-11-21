import XCTest
import CoreGraphics
import MTDSSKit

/// 缩略图内存缓存/装载器单测(V4-1 T24 fix round 1 Important #1):
/// daemon /thumb 应答仅 Content-Type 无缓存头,RFC 7234 下 NSURLCache 不得
/// 存储(URLCache 形同虚设)——唯一缓存面 = NSCache 按 URL 键(Coil 内存
/// 缓存同语义):同 URL 二次取 = 缓存命中**零二次网络调用**;异 URL 各拉;
/// 非 200/解码失败不落缓存(可重试);默认缓存帽锚。
final class ThumbImageLoaderTests: XCTestCase {

    /// 1×1 PNG(标准透明像素;CGImageSource 在 macOS/iOS 均可解码)。
    private static let png1x1 = Data(base64Encoded:
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
    )!

    /// 计数装载缝(锁保护;返回 1×1 PNG)。
    private final class CountingFetch: @unchecked Sendable {
        private let lock = NSLock()
        private var countStorage = 0
        private let data: Data
        private let status: Int
        var count: Int { lock.withLock { countStorage } }
        init(data: Data, status: Int = 200) {
            self.data = data
            self.status = status
        }
        func call() -> @Sendable (URL) async throws -> (Data, URLResponse) {
            { [self] url in
                lock.withLock { countStorage += 1 }
                // URL 恒合法,failable init 不会失败
                let response = HTTPURLResponse(url: url, statusCode: status, httpVersion: nil, headerFields: nil)!
                return (data, response)
            }
        }
    }

    private func url(_ string: String) -> URL {
        URL(string: string)!
    }

    // MARK: 同 URL 二次取 = 缓存命中,零二次网络调用(fix round 1 钉面)

    func testSameUrlSecondHitServedFromCacheWithoutRefetch() async {
        let fetch = CountingFetch(data: Self.png1x1)
        let loader = ThumbImageLoader(fetch: fetch.call())
        let u = url("http://127.0.0.1:39801/f/h1/thumb?w=256&t=tok")
        let first = await loader.image(for: u)
        XCTAssertNotNil(first, "首拉解码成图")
        let second = await loader.image(for: u)
        XCTAssertTrue(second != nil && second === first, "同 URL 二次取 = 缓存同实例")
        XCTAssertEqual(fetch.count, 1, "二次命中缓存,零二次网络调用")
    }

    func testDifferentUrlsFetchSeparately() async {
        let fetch = CountingFetch(data: Self.png1x1)
        let loader = ThumbImageLoader(fetch: fetch.call())
        _ = await loader.image(for: url("http://127.0.0.1:1/f/a/thumb?w=256&t=ta"))
        _ = await loader.image(for: url("http://127.0.0.1:1/f/b/thumb?w=256&t=tb"))
        XCTAssertEqual(fetch.count, 2, "异 URL 各拉各存")
    }

    // MARK: 非 200 不落缓存(失败面可重试)

    func testNon200NotCachedAndRetriesOnNextCall() async {
        let fetch = CountingFetch(data: Data(), status: 404)
        let loader = ThumbImageLoader(fetch: fetch.call())
        let u = url("http://127.0.0.1:1/f/gone/thumb?w=256&t=t")
        let first = await loader.image(for: u)
        XCTAssertNil(first, "非 200 不出图")
        let second = await loader.image(for: u)
        XCTAssertNil(second)
        XCTAssertEqual(fetch.count, 2, "失败面不落缓存:二次调用重发(可重试语义)")
    }

    // MARK: 解码失败不落缓存

    func testDecodeFailureNotCachedAndRetriesOnNextCall() async {
        let fetch = CountingFetch(data: Data("not an image".utf8), status: 200)
        let loader = ThumbImageLoader(fetch: fetch.call())
        let u = url("http://127.0.0.1:1/f/bad/thumb?w=256&t=t")
        let first = await loader.image(for: u)
        XCTAssertNil(first, "垃圾字节不解码")
        _ = await loader.image(for: u)
        XCTAssertEqual(fetch.count, 2, "解码失败不落缓存,二次重发")
    }

    // MARK: 默认缓存帽锚(会话内复用面)

    func testDefaultCacheCountLimitAnchor() {
        let loader = ThumbImageLoader()
        XCTAssertEqual(loader.cacheCountLimit, 256, "默认缓存帽 = 256 枚(NSCache 内存压力自逐)")
    }

    // MARK: 解码纯函数(1×1 PNG 真解码)

    func testDecodePngProducesCGImage() {
        let img = ThumbImageLoader.decode(Self.png1x1)
        XCTAssertNotNil(img, "CGImageSource 解码 1×1 PNG")
        XCTAssertEqual(img?.width, 1)
    }

    func testDecodeGarbageYieldsNil() {
        XCTAssertNil(ThumbImageLoader.decode(Data("junk".utf8)))
    }
}
