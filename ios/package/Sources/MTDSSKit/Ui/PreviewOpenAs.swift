/// 「用其他应用打开」整件取数(V4-1 T25 fix round 1 Important #2):流式
/// 落盘收口。传输契约 = URL → 临时件 URL(downloadTask 文件搬运,**零
/// Data 全量缓冲面**——大件 mkv/avi 等打开器主场景 jetsam 风险,Data 形
/// 数据面在签名级不存在)。目录自净/净化名/搬运收口在本面,宿主单测直驱
/// (PreviewOpenAsTests):注入假传输器钉 目录自净 / 净化名搬运 / 传输错
/// 透传 / 出站 URL 逐字。

import Foundation

public struct PreviewOpenAsFetcher: Sendable {

    /// 传输器契约:回环 URL → 已落盘临时件 URL(downloadTask 流式;临时件
    /// 由系统暂存,校验须在回调内完成——回调返回后系统即清临时件)。
    public typealias Transport = @Sendable (URL) async throws -> URL

    /// 缓存子目录名({caches}/preview-open;打开器只消费当次拷贝)。
    public static let directoryName = "preview-open"

    /// 流式传输会话(ephemeral 无缓存——句柄 URL 短命不落盘;15s 请求空闲
    /// 超时:流式到达即重置,长件不截断)。
    static let session: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.urlCache = nil
        config.requestCachePolicy = .reloadIgnoringLocalCacheData
        config.timeoutIntervalForRequest = 15
        return URLSession(configuration: config)
    }()

    /// 缺省传输器:downloadTask 文件搬运 + HTTP 200 校验(校验在回调内——
    /// downloadTask 临时件回调返回后即被系统清理,非 200 件不交接)。非
    /// 200 = RpcError「HTTP <code>」(非离线族词,调用方「打开失败:」前缀
    /// 直透语义不受影响)。
    public static func urlSessionTransport() -> Transport {
        { url in
            try await withCheckedThrowingContinuation { cont in
                let task = session.downloadTask(with: url) { location, response, error in
                    if let error {
                        cont.resume(throwing: error)
                        return
                    }
                    let code = (response as? HTTPURLResponse)?.statusCode ?? -1
                    guard let location, code == 200 else {
                        cont.resume(throwing: RpcError(code: 1, message: "HTTP \(code)"))
                        return
                    }
                    cont.resume(returning: location)
                }
                task.resume()
            }
        }
    }

    private let transport: Transport
    private let cachesDirectory: URL
    private let nowMs: @Sendable () -> Int64

    /// - Parameters:
    ///   - transport: 传输缝(nil = ``urlSessionTransport()`` 流式件;测试
    ///     注入假件——契约即文件搬运)。
    ///   - cachesDirectory: 缓存根(nil = 系统 caches;测试注入临时目录)。
    ///   - nowMs: 时间戳缝(净化名前缀;测试注入定格)。
    ///   默认值装配在 init 体内(public init 默认参不得引 internal 符号,
    ///   T24 先案)。
    public init(
        transport: Transport? = nil,
        cachesDirectory: URL? = nil,
        nowMs: @escaping @Sendable () -> Int64 = { Int64(Date().timeIntervalSince1970 * 1000) },
    ) {
        self.transport = transport ?? Self.urlSessionTransport()
        self.cachesDirectory = cachesDirectory
            ?? FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        self.nowMs = nowMs
    }

    /// 整件取数:目录自净(旧件不留)→ 流式传输 → 搬运净化名目标位。返回
    /// 值即文件 URL(扩展名保真 = 系统打开器路由依据;自净先于传输 = 传输
    /// 失败也不留旧件堆积)。
    public func fetch(entryName: String, rawUrl: URL) async throws -> URL {
        let dir = cachesDirectory.appendingPathComponent(Self.directoryName, isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        // 目录自净:打开器只消费当次拷贝,旧件不留(安卓 preview-open 同象)
        for old in try FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil) {
            try FileManager.default.removeItem(at: old)
        }
        let file = dir.appendingPathComponent(Self.sanitizedName(entryName, stampMs: nowMs()))
        let temp = try await transport(rawUrl)
        // 搬运目标位(临时件 → 净化名;同位已存先清,move 幂等容错)
        if FileManager.default.fileExists(atPath: file.path) {
            try FileManager.default.removeItem(at: file)
        }
        try FileManager.default.moveItem(at: temp, to: file)
        return file
    }

    /// 净化名:`<stampMs>-<净化原名>`——非法路径字符 `[\\/:*?"<>|]` 落下划线,
    /// 扩展名保真(打开器路由依据),名字段尾截 120(安卓 takeLast(120)
    /// 同构,时间戳前缀防同目录重名)。
    public static func sanitizedName(_ name: String, stampMs: Int64) -> String {
        let safe = name.replacingOccurrences(
            of: "[\\\\/:*?\"<>|]",
            with: "_",
            options: .regularExpression,
        )
        return "\(stampMs)-\(String(safe.suffix(120)))"
    }
}
