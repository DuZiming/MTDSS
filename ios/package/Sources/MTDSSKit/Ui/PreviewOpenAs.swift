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
    /// r54 #4:增进度缝(已写字节, 总字节;Content-Length 缺席总字节 0)。
    public typealias Transport = @Sendable (URL, Progress?) async throws -> URL

    /// 进度缝:已写 / 总字节(总 0 = 未知,调用面按不确定态呈现)。
    public typealias Progress = @Sendable (Int64, Int64) -> Void

    /// 缓存子目录名({caches}/preview-open;打开器只消费当次拷贝)。
    public static let directoryName = "preview-open"

    /// 缺省传输器(r54 #4:进度经 downloadTask 代理面;**取消**走任务取消
    /// ——调用方 Task.cancel 经 withTaskCancellationHandler 落到
    /// downloadTask.cancel,URLError.cancelled 上抛)。非 200 = RpcError
    /// 「HTTP <code>」(校验在回调内——downloadTask 临时件回调返回后即被
    /// 系统清理,非 200 件不交接)。
    public static func urlSessionTransport() -> Transport {
        { url, onProgress in
            // 每取数一会话 + 代理(取数低频;finishTasksAndInvalidate 收口)
            final class ProgressDelegate: NSObject, URLSessionDownloadDelegate {
                let onProgress: Progress?
                let cont: CheckedContinuation<URL, Error>
                init(onProgress: Progress?, cont: CheckedContinuation<URL, Error>) {
                    self.onProgress = onProgress
                    self.cont = cont
                }
                func urlSession(
                    _ session: URLSession, downloadTask: URLSessionDownloadTask,
                    didWriteData: Int64, totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64
                ) {
                    // 未知总长面:系统可回传负值 → 归零(不确定态)
                    onProgress?(totalBytesWritten, max(0, totalBytesExpectedToWrite))
                }
                func urlSession(
                    _ session: URLSession, downloadTask: URLSessionDownloadTask,
                    didFinishDownloadingTo location: URL
                ) {
                    let code = (downloadTask.response as? HTTPURLResponse)?.statusCode ?? -1
                    guard code == 200 else {
                        cont.resume(throwing: RpcError(code: 1, message: "HTTP \(code)"))
                        return
                    }
                    cont.resume(returning: location)
                }
                func urlSession(
                    _ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?
                ) {
                    guard let error else { return } // 成功路径由 didFinish 面收
                    cont.resume(throwing: error)
                }
            }
            let config = URLSessionConfiguration.ephemeral
            config.urlCache = nil
            config.requestCachePolicy = .reloadIgnoringLocalCacheData
            config.timeoutIntervalForRequest = 15
            // 取消任务盒(创建竞窗:cancel 先于任务创建 = no-op,任务错误
            // 回调永不到来是既有险面——迟到 cancel 由 URL 会话收口兜底)
            final class TaskBox: @unchecked Sendable {
                var task: URLSessionDownloadTask?
            }
            let box = TaskBox()
            return try await withTaskCancellationHandler {
                try await withCheckedThrowingContinuation { (cont: CheckedContinuation<URL, Error>) in
                    let delegate = ProgressDelegate(onProgress: onProgress, cont: cont)
                    let session = URLSession(configuration: config, delegate: delegate, delegateQueue: nil)
                    let task = session.downloadTask(with: url)
                    box.task = task
                    session.finishTasksAndInvalidate()
                    task.resume()
                }
            } onCancel: {
                // 取消缝:调用方 Task.cancel → downloadTask.cancel(错误回调
                // 面上抛 URLError.cancelled)
                box.task?.cancel()
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

    /// 整件取数:目录自净(旧件不留;**B 案钉死**——每次清空重下,用户
    /// 2026-11-27 裁定)→ 流式传输(进度缝透传)→ 搬运净化名目标位。
    /// 返回值即文件 URL(扩展名保真 = 系统打开器路由依据;自净先于传输
    /// = 传输失败也不留旧件堆积)。
    public func fetch(
        entryName: String,
        rawUrl: URL,
        onProgress: Progress? = nil,
    ) async throws -> URL {
        let dir = cachesDirectory.appendingPathComponent(Self.directoryName, isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        // 目录自净:打开器只消费当次拷贝,旧件不留(安卓 preview-open 同象)
        for old in try FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil) {
            try FileManager.default.removeItem(at: old)
        }
        let file = dir.appendingPathComponent(Self.sanitizedName(entryName, stampMs: nowMs()))
        let temp = try await transport(rawUrl, onProgress)
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
