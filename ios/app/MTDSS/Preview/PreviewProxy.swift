/// 预览自管 scheme 代理 + 打开器壳(V4-1 T25;iOS WKWebView 载体面):
/// - ``PreviewProxySchemeHandler``:把 ``PreviewRoute/proxyScheme`` 做成
///   宿主页**同源面**——WebKit 无安卓 allowUniversalAccessFromFileURLs
///   等价开关,file:// 页 fetch 回环口恒被 CORS 拒(daemon 不发
///   Access-Control 头,零 daemon 改动纪律)。宿主页 + previewer bundle
///   资产 + 回环代理全走本 handler:同源 = fetch/XHR/Range 预检全零,
///   previewer bundle 零改动(桌面/安卓同 bundle 逐字复用);出站单点 =
///   ``PreviewRoute/proxyRequest(for:)``(Range 头转发,fix round 1 #1)。
/// - **r33 流式代理**:completion 式整体积缓冲在远端大视频上 = 主线程巨
///   Data → App 冻死/OOM(实机:开远端 PC 视频全卡)。改 delegate 分块
///   流:didReceive 逐块转发 WKURLSchemeTask,内存恒块级;playlist 的
///   m3u8 体改写在收齐后一次发(体量小)。
/// - ``ActivitySheet``:UIActivityViewController 壳(用其他应用打开落点;
///   仓内无系统分享先例,系统 sheet 即「用其他应用打开」通用面)。open-as
///   整件取数 = Kit ``PreviewOpenAsFetcher``(downloadTask 流式落盘,fix
///   round 1 #2 撤全量缓冲)。
///
/// WKURLSchemeTask 纪律:stop 之后不得再调 didReceive/didFinish(WebKit
/// NSException)——**emit 全程持锁**(stop 与数据回调两线串行,无
/// check-then-act 窗;stop 即摘作业,数据回调查无作业即弃发)。

import MTDSSKit
import SwiftUI
import UIKit
import WebKit

final class PreviewProxySchemeHandler: NSObject, WKURLSchemeHandler {

    /// 在飞作业(stop 标记 + 可取消底层任务;键 = task 身份)。每个 start
    /// 的 task 必注册(资产臂/未知路径臂同注册)。r33:流式面增缓冲与
    /// 响应头账(首块到达即发响应)。
    private final class Job {
        var dataTask: URLSessionDataTask?
        var stopped = false
        var responseSent = false
        /// playlist 改写累积面(mpegurl 收齐一次发;其余流式直发)。
        var playlistBuffer: Data?
        var isPlaylist = false
    }

    private let lock = NSLock()
    private var jobs: [ObjectIdentifier: Job] = [:]
    /// WKURLSchemeTask 弱引用注册表(流式 delegate 反向映射面;stop 即摘)。
    private var registry: [ObjectIdentifier: WeakSchemeTask] = [:]

    private struct WeakSchemeTask {
        weak var value: WKURLSchemeTask?
    }

    /// 回环代理会话(delegate 流式;r33 撤 completion 整体积面)。
    private lazy var session: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.urlCache = nil
        config.requestCachePolicy = .reloadIgnoringLocalCacheData
        config.timeoutIntervalForRequest = 15
        return URLSession(configuration: config, delegate: self, delegateQueue: nil)
    }()

    func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
        let key = ObjectIdentifier(task)
        let job = Job()
        lock.lock()
        jobs[key] = job
        registry[key] = WeakSchemeTask(value: task)
        lock.unlock()
        // 资产臂:宿主页 + previewer bundle(同源资产面,缺失 = 404 字面)
        if let asset = PreviewProxyAsset.match(task.request.url) {
            guard let data = asset.data else {
                respondOnce(task, key: key, status: 404, contentType: "text/plain", extra: [:],
                        data: Data("资源缺失".utf8))
                return
            }
            respondOnce(task, key: key, status: 200, contentType: asset.contentType, extra: [:], data: data)
            return
        }
        // 代理臂:daemon 回环口(出站单点 = [PreviewRoute/proxyRequest(for:)]
        // ——Range 头转发,fix round 1 Important #1:响应侧广告
        // Accept-Ranges: bytes,出站不携则 daemon 恒 200 全量,pdf.js 按
        // 206 校验即抛「Unexpected server response」)
        guard let outgoing = PreviewRoute.proxyRequest(for: task.request) else {
            respondOnce(task, key: key, status: 404, contentType: "text/plain", extra: [:],
                    data: Data("未知路径".utf8))
            return
        }
        job.dataTask = session.dataTask(with: outgoing)
        job.dataTask?.resume()
    }

    func webView(_ webView: WKWebView, stop task: WKURLSchemeTask) {
        let key = ObjectIdentifier(task)
        lock.lock()
        registry.removeValue(forKey: key)
        if let job = jobs.removeValue(forKey: key) {
            job.stopped = true
            job.dataTask?.cancel()
        }
        lock.unlock()
    }

    /// delegate 反查:dataTask → (scheme task, job, key)。锁内调用。
    private func lookup(_ dataTask: URLSessionTask) -> (WKURLSchemeTask, Job, ObjectIdentifier)? {
        for (key, job) in jobs where job.dataTask === dataTask {
            if let t = registry[key]?.value {
                return (t, job, key)
            }
            return nil
        }
        return nil
    }

    // MARK: r33 流式转发(URLSessionDataDelegate;全部回调在锁内查作业)

    private func emitResponse(_ task: WKURLSchemeTask, _ job: Job, _ key: ObjectIdentifier,
                              status: Int, contentType: String, extra: [String: String]) -> Bool {
        guard !job.stopped, !job.responseSent else { return job.stopped == false }
        var headers = extra
        headers["Content-Type"] = contentType
        // r36:上游 Content-Length 透传(extra 携带;缺席=分块传输)——
        // WebKit 媒体管线要总长才推缓冲(剥掉=mp4 直读卡第一帧根因);
        // 流式分块转发面不变,内存仍块级
        let url = task.request.url ?? URL(string: PreviewRoute.hostPageUrl)!
        let resp = HTTPURLResponse(url: url, statusCode: status, httpVersion: "HTTP/1.1",
                                   headerFields: headers)!
        task.didReceive(resp)
        job.responseSent = true
        return true
    }

    private func emitChunk(_ task: WKURLSchemeTask, _ job: Job, _ data: Data) {
        guard !job.stopped, job.responseSent else { return }
        task.didReceive(data)
    }

    private func emitFinish(_ task: WKURLSchemeTask, _ job: Job, _ key: ObjectIdentifier) {
        guard !job.stopped else { return }
        if job.responseSent {
            task.didFinish()
        }
        jobs.removeValue(forKey: key)
    }
}

extension PreviewProxySchemeHandler: URLSessionDataDelegate {
    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask,
                    didReceive response: URLResponse,
                    completionHandler: @escaping (URLSession.ResponseDisposition) -> Void) {
        // 找回 WKURLSchemeTask(反向映射:job 持 dataTask;线性查 — 代理
        // 并发量级 = 单页几十路,线性可容)
        guard let http = response as? HTTPURLResponse else {
            completionHandler(.cancel)
            return
        }
        lock.lock()
        let match = lookup(dataTask)
        lock.unlock()
        guard let (task, job, key) = match else {
            completionHandler(.cancel)
            return
        }
        var extra: [String: String] = ["Accept-Ranges": "bytes"]
        if let range = http.value(forHTTPHeaderField: "Content-Range") {
            extra["Content-Range"] = range
        }
        // r36:上游 Content-Length 透传(WebKit 媒体管线缓冲推进要总长;
        // daemon 200/206 响应恒携;缺席面=真流式,不强造)
        if let len = http.value(forHTTPHeaderField: "Content-Length") {
            extra["Content-Length"] = len
        }
        let contentType = http.value(forHTTPHeaderField: "Content-Type") ?? "application/octet-stream"
        // playlist 面:累积改写后一次发(体量小;改写需全文)
        if contentType.contains("mpegurl") {
            job.isPlaylist = true
            job.playlistBuffer = Data()
        }
        lock.lock()
        _ = emitResponse(task, job, key, status: http.statusCode, contentType: contentType, extra: extra)
        lock.unlock()
        completionHandler(.allow)
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        lock.lock()
        let match = lookup(dataTask)
        lock.unlock()
        guard let (task, job, key) = match else { return }
        if job.isPlaylist {
            lock.lock()
            job.playlistBuffer?.append(data)
            lock.unlock()
            return
        }
        lock.lock()
        emitChunk(task, job, data)
        lock.unlock()
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        lock.lock()
        let match = lookup(task)
        lock.unlock()
        guard let (wkTask, job, key) = match else { return }
        if let error = error as? URLError, error.code == .cancelled {
            return // stop 面取消:静默
        }
        if job.isPlaylist, let buffer = job.playlistBuffer {
            let rewritten = Self.rewritePlaylistOrigins(buffer)
            lock.lock()
            emitChunk(wkTask, job, rewritten)
            emitFinish(wkTask, job, key)
            lock.unlock()
            return
        }
        if error != nil {
            // 回环不可达(引擎退场/句柄过期):502 字面(若响应已开,只能
            // 硬终——WebKit 无 mid-stream 状态码改写面;错误由页自示)
            lock.lock()
            if !job.responseSent {
                _ = emitResponse(wkTask, job, key, status: 502, contentType: "text/plain", extra: [:])
                wkTask.didReceive(Data("预览源不可达".utf8))
            }
            emitFinish(wkTask, job, key)
            lock.unlock()
            return
        }
        lock.lock()
        emitFinish(wkTask, job, key)
        lock.unlock()
    }
}

extension PreviewProxySchemeHandler {
    /// r31:HLS playlist 源改写——`http://127.0.0.1:{port}` →
    /// `mtdsspreview://localhost/p/{port}`(端口数字保序;仅 m3u8 体消费,
    /// 段请求回代理臂闭环)。纯字节扫描无正则依赖。
    static func rewritePlaylistOrigins(_ body: Data) -> Data {
        guard let text = String(data: body, encoding: .utf8) else { return body }
        let marker = "http://127.0.0.1:"
        var out = ""
        var rest = Substring(text)
        while let range = rest.range(of: marker) {
            out += rest[rest.startIndex..<range.lowerBound]
            let after = rest[range.upperBound...]
            var digits = ""
            var iter = after.makeIterator()
            var consumed = ""
            while let c = iter.next(), c.isNumber {
                digits.append(c)
                consumed.append(c)
            }
            if !digits.isEmpty {
                out += "mtdsspreview://localhost/p/\(digits)"
                rest = after.dropFirst(consumed.count)
            } else {
                out += marker
                rest = after
            }
        }
        out += rest
        return Data(out.utf8)
    }

    // MARK: 单发应答(资产臂/错误面;原 completion 语义)

    private func respondOnce(
        _ task: WKURLSchemeTask, key: ObjectIdentifier, status: Int,
        contentType: String, extra: [String: String] = [:], data: Data
    ) {
        var headers = extra
        headers["Content-Type"] = contentType
        headers["Content-Length"] = "\(data.count)"
        let url = task.request.url ?? URL(string: PreviewRoute.hostPageUrl)
        let resp = url.flatMap {
            HTTPURLResponse(url: $0, statusCode: status, httpVersion: "HTTP/1.1", headerFields: headers)
        } ?? URLResponse(url: url ?? URL(fileURLWithPath: "/"), mimeType: contentType,
                         expectedContentLength: data.count, textEncodingName: nil)
        lock.lock()
        defer { lock.unlock() }
        guard jobs[key] != nil else { return } // stop 已摘 = WebKit 契约禁再调
        jobs.removeValue(forKey: key)
        task.didReceive(resp)
        task.didReceive(data)
        task.didFinish()
    }
}

/// 资产臂路由:宿主页/previewer bundle → App bundle resource(folder
/// reference 整目录进包,`previewer/` 子目录直取;数据缺失 = nil,调用臂
/// 404)。其余路径 = nil(调用臂走代理反解)。
private enum PreviewProxyAsset {
    static func match(_ url: URL?) -> (data: Data?, contentType: String)? {
        guard let url else { return nil }
        switch url.path(percentEncoded: true) {
        case "/preview.html":
            return (bundleData(name: "preview", ext: "html"), "text/html; charset=utf-8")
        case "/previewer.js":
            return (bundleData(name: "previewer", ext: "js"), "text/javascript; charset=utf-8")
        case "/previewer.css":
            // field r24 #3:CSS 资产臂补——此前 404 落代理臂「未知路径」,
            // 文档容器全 UI 未样式化(桌面 r14 同族根因)。
            return (bundleData(name: "previewer", ext: "css"), "text/css; charset=utf-8")
        default:
            return nil
        }
    }

    private static func bundleData(name: String, ext: String) -> Data? {
        guard let url = Bundle.main.url(forResource: name, withExtension: ext, subdirectory: "previewer") else {
            return nil
        }
        return try? Data(contentsOf: url)
    }
}

/// 「用其他应用打开」落点(UIActivityViewController 壳;点开系统分享面,
/// 用户选任意外部 App;完成即收 sheet)。
struct ActivitySheet: UIViewControllerRepresentable {
    let url: URL
    let onDone: () -> Void

    func makeUIViewController(context: Context) -> UIActivityViewController {
        let controller = UIActivityViewController(activityItems: [url], applicationActivities: nil)
        controller.completionWithItemsHandler = { _, _, _, _ in onDone() }
        return controller
    }

    func updateUIViewController(_ uiViewController: UIActivityViewController, context: Context) {}
}
