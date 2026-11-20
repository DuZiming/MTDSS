import XCTest
@testable import MTDSSKit

/// 「用其他应用打开」整件取数宿主单测(V4-1 T25 fix round 1 Important #2;
/// PreviewOpenAsFetcher):传输契约 = URL → 临时件 URL(downloadTask 流式,
/// **Data 形全量缓冲面签名级不存在**——注入缝钉「文件搬运路径」):目录
/// 自净 / 净化名保扩展名 / 出站 URL 逐字 / 传输错透传且目录已自净。
final class PreviewOpenAsTests: XCTestCase {

    private var tempRoot: URL!

    override func setUpWithError() throws {
        tempRoot = FileManager.default.temporaryDirectory
            .appendingPathComponent("preview-open-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempRoot, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: tempRoot)
    }

    private var dir: URL {
        tempRoot.appendingPathComponent(PreviewOpenAsFetcher.directoryName, isDirectory: true)
    }

    private func makeFetcher(
        stampMs: Int64 = 1_700_000_000_000,
        transport: PreviewOpenAsFetcher.Transport?,
    ) -> PreviewOpenAsFetcher {
        PreviewOpenAsFetcher(transport: transport, cachesDirectory: tempRoot, nowMs: { stampMs })
    }

    private func createStaleFile(_ name: String) throws -> URL {
        // 目录自净先于 fetch 创建(fetch 内建目录)——预置旧件须自建目录
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let url = dir.appendingPathComponent(name)
        try Data("stale".utf8).write(to: url)
        return url
    }

    // MARK: 净化名(时间戳前缀 + 非法字符落下划线 + 扩展名保真)

    func testSanitizedNameKeepsExtensionAndStamps() {
        XCTAssertEqual(
            PreviewOpenAsFetcher.sanitizedName("movie.mkv", stampMs: 5),
            "5-movie.mkv",
            "扩展名保真 = 系统打开器路由依据",
        )
        XCTAssertEqual(
            PreviewOpenAsFetcher.sanitizedName("bad:name?.mkv", stampMs: 5),
            "5-bad_name_.mkv",
            "非法路径字符 [\\/:*?\"<>|] 落下划线",
        )
    }

    func testSanitizedNameTruncatesNameFieldOnlyKeepsSuffix() {
        let long = String(repeating: "长", count: 200) + ".pdf"
        let sanitized = PreviewOpenAsFetcher.sanitizedName(long, stampMs: 5)
        XCTAssertTrue(sanitized.hasSuffix(".pdf"), "尾截只截名字段,扩展名恒保真")
        XCTAssertEqual(sanitized.count, 2 + 120, "时间戳前缀 + 名字段 120 尾截(安卓同构)")
    }

    // MARK: fetch = 文件搬运路径(注入假传输器钉出站/自净/搬运)

    func testFetchMovesTempToSanitizedPathAndCleansDirectory() async throws {
        let stale = try createStaleFile("old.txt")
        var receivedUrl: URL?
        let payload = Data("payload".utf8)
        let fetcher = makeFetcher { url in
            receivedUrl = url
            let temp = FileManager.default.temporaryDirectory
                .appendingPathComponent("fake-\(UUID().uuidString)-movie.mkv")
            try payload.write(to: temp)
            return temp
        }
        let raw = URL(string: "http://127.0.0.1:8000/f/h1/raw?t=tk")!
        let file = try await fetcher.fetch(entryName: "movie.mkv", rawUrl: raw)
        // 出站 URL 逐字;落位 = 净化名(时间戳前缀 + 保扩展名)
        XCTAssertEqual(receivedUrl, raw)
        XCTAssertEqual(file.lastPathComponent, "1700000000000-movie.mkv")
        // 文件搬运语义:目标存在、内容原样、源临时件已随 move 消失
        XCTAssertEqual(try Data(contentsOf: file), payload)
        XCTAssertTrue(FileManager.default.fileExists(atPath: file.path))
        // 目录自净:旧件已清
        XCTAssertFalse(FileManager.default.fileExists(atPath: stale.path))
    }

    func testFetchTransportErrorPropagatesAndDirectoryAlreadyCleaned() async throws {
        let stale = try createStaleFile("old.mkv")
        let fetcher = makeFetcher { _ in
            throw RpcError(code: 1, message: "HTTP 500")
        }
        do {
            _ = try await fetcher.fetch(
                entryName: "movie.mkv",
                rawUrl: URL(string: "http://127.0.0.1:8000/f/h1/raw?t=tk")!,
            )
            XCTFail("传输错必透传")
        } catch let error as RpcError {
            XCTAssertEqual(error.message, "HTTP 500")
        }
        // 自净先于传输:传输失败不留旧件堆积
        XCTAssertFalse(FileManager.default.fileExists(atPath: stale.path))
    }

    func testFetchOverwritesSameNameWithoutMoveFailure() async throws {
        // 同净化名已存(同秒重开同名件)= 先清后搬,move 不因占位失败
        let stampMs: Int64 = 42
        let existing = try createStaleFile("\(stampMs)-movie.mkv")
        let payload = Data("fresh".utf8)
        let fetcher = makeFetcher(stampMs: stampMs) { url in
            let temp = FileManager.default.temporaryDirectory
                .appendingPathComponent("fake-\(UUID().uuidString)-movie.mkv")
            try payload.write(to: temp)
            return temp
        }
        let file = try await fetcher.fetch(
            entryName: "movie.mkv",
            rawUrl: URL(string: "http://127.0.0.1:8000/f/h1/raw?t=tk")!,
        )
        XCTAssertEqual(file, existing, "同位复用(先清后搬)")
        XCTAssertEqual(try Data(contentsOf: file), payload, "旧件被当次拷贝覆盖")
    }
}
