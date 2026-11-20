/// 预览自管 scheme 代理 + 打开器壳(V4-1 T25;iOS WKWebView 载体面):
/// - ``PreviewProxySchemeHandler``:把 ``PreviewRoute/proxyScheme`` 做成
///   宿主页**同源面**——WebKit 无安卓 allowUniversalAccessFromFileURLs
///   等价开关,file:// 页 fetch 回环口恒被 CORS 拒(daemon 不发
///   Access-Control 头,零 daemon 改动纪律)。宿主页 + previewer bundle
///   资产 + 回环代理全走本 handler:同源 = fetch/XHR/Range 预检全零,
///   previewer bundle 零改动(桌面/安卓同 bundle 逐字复用)。
/// - ``PreviewHttpSession``:App 侧回环取数会话(ephemeral 无缓存;open-as
///   整件拷取;15s 超时同 ThumbSession 口径)。
/// - ``ActivitySheet``:UIActivityViewController 壳(用其他应用打开落点;
///   仓内无系统分享先例,系统 sheet 即「用其他应用打开」通用面)。
///
/// WKURLSchemeTask 纪律:stop 之后不得再调 didReceive/didFinish(WebKit
/// NSException)——**emit 全程持锁**(stop 与完成回调两线串行,无
/// check-then-act 窗;stop 即摘作业,完成回调查无作业即弃发);URLSession
/// 无 delegate(WKURLSchemeHandler 硬约束,自带 delegate 即 NSException)。

import MTDSSKit
import SwiftUI
import UIKit
import WebKit

final class PreviewProxySchemeHandler: NSObject, WKURLSchemeHandler {

    /// 在飞作业(stop 标记 + 可取消底层任务;键 = task 身份)。每个 start
    /// 的 task 必注册(资产臂/未知路径臂同注册,respond 收口单点)。
    private final class Job {
        var dataTask: URLSessionDataTask?
        var stopped = false
    }

    private let lock = NSLock()
    private var jobs: [ObjectIdentifier: Job] = [:]

    /// 回环代理会话(ephemeral 无缓存;无 delegate = WKURLSchemeHandler
    /// 硬约束;15s 超时同 ThumbSession 口径)。
    private let session: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.urlCache = nil
        config.requestCachePolicy = .reloadIgnoringLocalCacheData
        config.timeoutIntervalForRequest = 15
        return URLSession(configuration: config)
    }()

    func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
        let key = ObjectIdentifier(task)
        let job = Job()
        lock.lock()
        jobs[key] = job
        lock.unlock()
        // 资产臂:宿主页 + previewer bundle(同源资产面,缺失 = 404 字面)
        if let asset = PreviewProxyAsset.match(task.request.url) {
            guard let data = asset.data else {
                respond(task, key: key, status: 404, contentType: "text/plain", extra: [:],
                        data: Data("资源缺失".utf8))
                return
            }
            respond(task, key: key, status: 200, contentType: asset.contentType, extra: [:], data: data)
            return
        }
        // 代理臂:daemon 回环口(出站单点 = [PreviewRoute/proxyRequest(for:)]
        // ——Range 头转发,fix round 1 Important #1:响应侧广告
        // Accept-Ranges: bytes,出站不携则 daemon 恒 200 全量,pdf.js 按
        // 206 校验即抛「Unexpected server response」)
        guard let outgoing = PreviewRoute.proxyRequest(for: task.request) else {
            respond(task, key: key, status: 404, contentType: "text/plain", extra: [:],
                    data: Data("未知路径".utf8))
            return
        }
        let dataTask = session.dataTask(with: outgoing) { [weak self] data, response, error in
            guard let self else { return }
            let taskKey = ObjectIdentifier(task)
            if let http = response as? HTTPURLResponse, error == nil {
                var extra: [String: String] = ["Accept-Ranges": "bytes"]
                // Range 面(pdf.js 段取):daemon 206 的 Content-Range 原样透传
                if let range = http.value(forHTTPHeaderField: "Content-Range") {
                    extra["Content-Range"] = range
                }
                self.respond(
                    task, key: taskKey, status: http.statusCode,
                    contentType: http.value(forHTTPHeaderField: "Content-Type") ?? "application/octet-stream",
                    extra: extra, data: data ?? Data())
            } else {
                // 回环不可达(引擎退场/句柄过期):502 字面,页面自示错误态
                self.respond(task, key: taskKey, status: 502, contentType: "text/plain", extra: [:],
                             data: Data("预览源不可达".utf8))
            }
        }
        lock.lock()
        job.dataTask = dataTask
        lock.unlock()
        dataTask.resume()
    }

    func webView(_ webView: WKWebView, stop task: WKURLSchemeTask) {
        lock.lock()
        if let job = jobs.removeValue(forKey: ObjectIdentifier(task)) {
            job.stopped = true
            job.dataTask?.cancel()
        }
        lock.unlock()
    }

    // MARK: 应答发射(作业查无/已停 = 弃发;emit 全程持锁防 stop 竞窗)

    private func respond(
        _ task: WKURLSchemeTask, key: ObjectIdentifier, status: Int,
        contentType: String, extra: [String: String], data: Data,
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

/// App 侧回环取数会话已撤(fix round 1 Important #2:open-as 整件取数迁
/// Kit ``PreviewOpenAsFetcher``——downloadTask 流式落盘,大件不整件入 RAM;
/// 代理臂的会话仍在本文件 handler 内)。

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
