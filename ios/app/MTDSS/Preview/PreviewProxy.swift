/// 预览自管 scheme 代理 + 打开器壳(V4-1 T25;iOS WKWebView 载体面):
/// - ``PreviewProxySchemeHandler``:把 ``PreviewRoute/proxyScheme`` 做成
///   宿主页**同源面**——WebKit 无安卓 allowUniversalAccessFromFileURLs
///   等价开关,file:// 页 fetch 回环口恒被 CORS 拒(daemon 不发
///   Access-Control 头,零 daemon 改动纪律)。宿主页 + previewer bundle
///   资产 + 回环代理全走本 handler:同源 = fetch/XHR/Range 预检全零,
///   previewer bundle 零改动(桌面/安卓同 bundle 逐字复用);出站单点 =
///   ``PreviewRoute/proxyRequest(for:)``(Range 头转发,fix round 1 #1)。
/// - **r33 流式 + r37 主线程串行**:completion 式整体积缓冲在远端大视频
///   上 = 主线程巨 Data → App 冻死/OOM(r33 改 delegate 分块流)。r33 的
///   流式回调在 URLSession delegate 队列(后台)直调
///   ``WKURLSchemeTask/didReceive`` ——**违反 WebKit 铁律(scheme task
///   调用必须在主线程)**:后台持锁调 WebKit + 主线程 stop() 抢锁 = 死锁
///   冻结;时序错开 = NSException 崩溃(实机:mp4 直读卡死→崩溃双形态)。
///   r37 修 = **全部状态与 WebKit 调用收主线程**:delegate 回调携原始
///   载荷 `DispatchQueue.main.async` 跳主,start/stop/emit 主线程天然
///   串行 → 锁整个撤除,use-after-stop 竞窗同消。
/// - ``ActivitySheet``:UIActivityViewController 壳(用其他应用打开落点)。
///
/// WKURLSchemeTask 纪律:stop 之后不得再调 didReceive/didFinish(WebKit
/// NSException)——r37 起由主线程串行保证(stop 与 emit 同队列,先到先
/// 行;stopped 旗在摘除时置位,后到 emit 查无作业即弃)。

import MTDSSKit
import SwiftUI
import UIKit
import WebKit

final class PreviewProxySchemeHandler: NSObject, WKURLSchemeHandler {

    /// 在飞作业(主线程 confinement;键 = task 身份)。
    private final class Job {
        var dataTask: URLSessionDataTask?
        var responseSent = false
        /// playlist 改写累积面(mpegurl 收齐一次发;其余流式直发)。
        var playlistBuffer: Data?
        var isPlaylist = false
    }

    /// 全部主线程访问(r37:start/stop = WebKit 主线程回调;emit = 主队列
    /// dispatch;零锁)。
    private var jobs: [ObjectIdentifier: Job] = [:]
    /// WKURLSchemeTask 弱引用注册表(delegate 反向映射面;stop 即摘)。
    private var registry: [ObjectIdentifier: WeakSchemeTask] = [:]

    private struct WeakSchemeTask {
        weak var value: WKURLSchemeTask?
    }

    /// 回环代理会话(delegate 流式;回调跳主线程,见类注 r37)。
    /// r42(用户裁定「300s 超时再报失败」):请求空闲窗 15s → 300s——大
    /// PDF 首取 2MB/大视频首块在慢网中继下超 15s 即断 = 文档族「加载失
    /// 败」误杀根因;资源总窗 600s 兜(等不及用户自退同视频面裁定)。
    private lazy var session: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.urlCache = nil
        config.requestCachePolicy = .reloadIgnoringLocalCacheData
        config.timeoutIntervalForRequest = 300
        config.timeoutIntervalForResource = 600
        return URLSession(configuration: config, delegate: self, delegateQueue: nil)
    }()

    func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
        let key = ObjectIdentifier(task)
        // 资产臂:宿主页 + previewer bundle(同源资产面,缺失 = 404 字面)
        if let asset = PreviewProxyAsset.match(task.request.url) {
            guard let data = asset.data else {
                respondOnce(task, status: 404, contentType: "text/plain",
                            data: Data("资源缺失".utf8))
                return
            }
            respondOnce(task, status: 200, contentType: asset.contentType, data: data)
            return
        }
        // 代理臂:daemon 回环口(出站单点 = [PreviewRoute/proxyRequest(for:)]
        // ——Range 头转发,fix round 1 Important #1:响应侧广告
        // Accept-Ranges: bytes,出站不携则 daemon 恒 200 全量,pdf.js 按
        // 206 校验即抛「Unexpected server response」)
        guard let outgoing = PreviewRoute.proxyRequest(for: task.request) else {
            respondOnce(task, status: 404, contentType: "text/plain",
                        data: Data("未知路径".utf8))
            return
        }
        let job = Job()
        jobs[key] = job
        registry[key] = WeakSchemeTask(value: task)
        job.dataTask = session.dataTask(with: outgoing)
        job.dataTask?.resume()
    }

    func webView(_ webView: WKWebView, stop task: WKURLSchemeTask) {
        let key = ObjectIdentifier(task)
        registry.removeValue(forKey: key)
        if let job = jobs.removeValue(forKey: key) {
            job.dataTask?.cancel()
        }
    }

    // MARK: r37 主线程 emit 面(全部调用点已在主线程:webkit 回调 / 主队列跳)

    private func lookup(_ dataTask: URLSessionTask) -> (WKURLSchemeTask, Job, ObjectIdentifier)? {
        for (key, job) in jobs where job.dataTask === dataTask {
            guard let t = registry[key]?.value else { return nil }
            return (t, job, key)
        }
        return nil
    }

    private func emitResponse(
        _ task: WKURLSchemeTask, _ job: Job,
        status: Int, contentType: String, extra: [String: String]
    ) {
        guard !job.responseSent else { return }
        var headers = extra
        headers["Content-Type"] = contentType
        // r36:上游 Content-Length 透传(extra 携带;缺席=分块传输)——
        // WebKit 媒体管线要总长才推缓冲;流式分块转发面不变,内存仍块级
        let url = task.request.url ?? URL(string: PreviewRoute.hostPageUrl)!
        let resp = HTTPURLResponse(url: url, statusCode: status, httpVersion: "HTTP/1.1",
                                   headerFields: headers)!
        task.didReceive(resp)
        job.responseSent = true
    }

    private func emitChunk(_ task: WKURLSchemeTask, _ job: Job, _ data: Data) {
        guard job.responseSent else { return }
        task.didReceive(data)
    }

    private func emitFinish(_ task: WKURLSchemeTask, _ key: ObjectIdentifier) {
        task.didFinish()
        jobs.removeValue(forKey: key)
    }
}

extension PreviewProxySchemeHandler: URLSessionDataDelegate {
    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask,
                    didReceive response: URLResponse,
                    completionHandler: @escaping (URLSession.ResponseDisposition) -> Void) {
        guard let http = response as? HTTPURLResponse else {
            completionHandler(.cancel)
            return
        }
        var extra: [String: String] = ["Accept-Ranges": "bytes"]
        if let range = http.value(forHTTPHeaderField: "Content-Range") {
            extra["Content-Range"] = range
        }
        // r36:上游 Content-Length 透传(WebKit 媒体管线缓冲推进要总长)
        if let len = http.value(forHTTPHeaderField: "Content-Length") {
            extra["Content-Length"] = len
        }
        let contentType = http.value(forHTTPHeaderField: "Content-Type") ?? "application/octet-stream"
        let isPlaylist = contentType.contains("mpegurl")
        let status = http.statusCode
        DispatchQueue.main.async { [weak self] in
            guard let self, let (task, job, _) = self.lookup(dataTask) else { return }
            if isPlaylist {
                job.isPlaylist = true
                job.playlistBuffer = Data()
            }
            self.emitResponse(task, job, status: status, contentType: contentType, extra: extra)
        }
        completionHandler(.allow)
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        DispatchQueue.main.async { [weak self] in
            guard let self, let (task, job, _) = self.lookup(dataTask) else { return }
            if job.isPlaylist {
                job.playlistBuffer?.append(data)
                return
            }
            self.emitChunk(task, job, data)
        }
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        let cancelled = (error as? URLError)?.code == .cancelled
        let failed = error != nil
        DispatchQueue.main.async { [weak self] in
            guard let self, let (wkTask, job, key) = self.lookup(task) else { return }
            guard self.jobs[key] != nil else { return }
            if cancelled { return } // stop 面取消:静默
            if job.isPlaylist, let buffer = job.playlistBuffer {
                let rewritten = Self.rewritePlaylistOrigins(buffer)
                self.emitChunk(wkTask, job, rewritten)
                self.emitFinish(wkTask, key)
                return
            }
            if failed {
                // 回环不可达(引擎退场/句柄过期):502 字面(响应未开面;
                // 已开 = 只能硬终,错误由页自示)
                if !job.responseSent {
                    self.emitResponse(wkTask, job, status: 502, contentType: "text/plain", extra: [:])
                    wkTask.didReceive(Data("预览源不可达".utf8))
                }
                self.emitFinish(wkTask, key)
                return
            }
            self.emitFinish(wkTask, key)
        }
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

    // MARK: 单发应答(资产臂/错误面;webView(start:) = 主线程直调)

    private func respondOnce(
        _ task: WKURLSchemeTask, status: Int,
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
        let path = url.path(percentEncoded: true)
        switch path {
        case "/preview.html":
            return (bundleData(name: "preview", ext: "html"), "text/html; charset=utf-8")
        case "/previewer.js":
            return (bundleData(name: "previewer", ext: "js"), "text/javascript; charset=utf-8")
        case "/previewer.css":
            // field r24 #3:CSS 资产臂补——此前 404 落代理臂「未知路径」,
            // 文档容器全 UI 未样式化(桌面 r14 同族根因)。
            return (bundleData(name: "previewer", ext: "css"), "text/css; charset=utf-8")
        default:
            break
        }
        // r47 wasm 资产臂:pdf.js v6 图像解码器伴随资产(/wasm/<名>)——
        // 主线程取数形(getDocument useWorkerFetch:false)页面同源 fetch
        // 此面;App bundle previewer/wasm/ 目录(Xcode folder reference
        // 整目录进包)。MIME:wasm=application/wasm,JS 回退=text/javascript
        // (wasm 实例化失败时 pdf.js 才落 JS 回退——主线程 dynamic import
        // 同源面通)。
        if path.hasPrefix("/wasm/"), !path.contains("..") {
            let name = String(path.dropFirst("/wasm/".count))
            let ext = (name as NSString).pathExtension.lowercased()
            let mime = ext == "wasm" ? "application/wasm" : "text/javascript; charset=utf-8"
            return (bundleData(sub: "previewer/wasm", name: name), mime)
        }
        return nil
    }

    private static func bundleData(name: String, ext: String) -> Data? {
        guard let url = Bundle.main.url(forResource: name, withExtension: ext, subdirectory: "previewer") else {
            return nil
        }
        return try? Data(contentsOf: url)
    }

    /// wasm 子目录伴生资产(r47)。
    private static func bundleData(sub: String, name: String) -> Data? {
        guard let url = Bundle.main.url(forResource: (name as NSString).deletingPathExtension,
                                        withExtension: (name as NSString).pathExtension,
                                        subdirectory: sub) else {
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
