/// 缩略图桥应答器(V4-1 T11 预览波;daemon 侧 jobs.rs ThumbBridge /
/// fdpass 0x0B 契约为真源,Android ThumbResponder.kt 同构):消费 daemon
/// 的 `fs.request_thumb {request_id, path, w}` 事件,按 path 分流照片臂/
/// 文件臂,经 ``ThumbEncoder`` 生成 ≤w 宽 JPEG 字节,经 fdpass **thumb
/// 通道**(0x0B,单实例)回 `{jpeg_b64, ok, request_id}`(字母序,
/// field r26f 修3 内联 JPEG——撤临时文件三跳,I/O 面零文件;引擎读字节
/// 入源端缓存)(缩略图缺位只是降级:拒答时引擎落既有回退,不回错误帧)。
///
/// - **路径分流**(与 ``SeekResponder`` 同面,``PhotoVpath`` 共享单点防
///   漂移):`/照片图库` 前缀 → **照片臂**(相册/名 → ``PhotoLibraryService/
///   albums``+``PhotoLibraryService/assets`` 批内唯一名重放(PhotoResponder
///   同序同函数)解析 photoId → `openSeekable` 自有只读 fd);其余 →
///   **文件臂**(iOS 授权根 = 沙箱真实路径——收件箱 Documents/信任根,
///   ``DefaultSeekFiles`` open(2) 直开,目录 fail-closed「不可出图」)。
///   根自身/目录/缺席/未授权一律拒答(0x0B `{error, ok:false, request_id}`
///   字母序,无 path)。
/// - **降采样参数**:w 钳制 1...``thumbMaxWidth``(与 daemon
///   THUMB_MAX_WIDTH 同值)后原样到达编码器;视频/图片分型凭据 = 显示名
///   扩展名(编码器内部分型)。
/// - **源生命周期**:解码窗即源生命周期——编码后关 fd(``SeekResponder``
///   交付即关同哲学;生成失败面同样关闭,不泄漏)。
/// - **生成件生命周期**:r26f 修3 起内联交付,零落盘;`thumb-serve` 目录
///   仅存历史版本残件——**挂载清扫**清空兜底(目录与引擎缓存目录
///   thumb-tmp 分立——清扫整目录清空,同目录会抹掉引擎 LRU 缓存,daemon
///   preview::thumb.rs 模块注钉死;Android ThumbResponder 同纪律)。
/// - **编码面**:``ThumbEncoder`` 注入缝(宿主单测注入伪编码器——macOS
///   宿主不可触 AVAssetImageGenerator/UIImage 真解码,解码正确性归实机
///   冒烟;生产 = App 目标 IOSThumbEncoder,Ruling E 同款绑定归 App)。
/// - **生命周期**:App boot 时随 EventHub 订阅流 `run` 起来(事件流终结
///   即退,与引擎同寿;PhotoResponder 同款——挂载即清扫)。畸形事件帧在
///   解析面丢弃(类型化事件无缺字段形态);应答帧 ack ok:false = 请求已
///   超时/未知,静默收口(等待面已摘,事实以引擎侧读面为准)。

import Foundation

/// Concurrent gate (field r26f fix1: MainActor Task concurrency cap).
/// field r26g 补:drain()——事件流终结后等在飞任务归还(permits 回满),
/// run 收尾不弃在飞应答(交付完整性与测试确定性)。
public actor ConcurrentGate {
    private let limit: Int
    private var permits: Int
    private var waiters: [CheckedContinuation<Void, Never>] = []
    private var drainers: [CheckedContinuation<Void, Never>] = []

    public init(limit: Int) {
        self.limit = limit
        permits = limit
    }

    public func wait() async {
        if permits > 0 {
            permits -= 1
        } else {
            await withCheckedContinuation { c in waiters.append(c) }
        }
    }

    public func signal() {
        if waiters.isEmpty {
            permits += 1
            if permits == limit, !drainers.isEmpty {
                let ds = drainers
                drainers = []
                ds.forEach { $0.resume() }
            }
        } else {
            let cont = waiters.removeFirst()
            cont.resume()
        }
    }

    /// 等全部在飞任务归还(permits 回满 = 零在飞;已在满档 = 立即返)。
    public func drain() async {
        guard permits < limit else { return }
        await withCheckedContinuation { c in drainers.append(c) }
    }
}

/// Photo name->photoId pre-built index (field r26f fix2: replaces O(n) replay per request).
/// field r26g 补:值携 raw 显示名(去 ` (n)` 顺延后缀的原始名——
/// Android ThumbSource.displayName = asset.name 同面;编码器分型/测试钉面)。
public final class PhotoNameIndex: @unchecked Sendable {
    private let lock = NSLock()
    private var cache: [String: (stamp: Date, map: [String: (id: String, raw: String)])] = [:]
    private let ttl: TimeInterval = 15

    public func map(for album: String, assets: [PhotoAsset]) -> [String: (id: String, raw: String)] {
        lock.lock()
        if let hit = cache[album], Date().timeIntervalSince(hit.stamp) < ttl {
            let m = hit.map
            lock.unlock()
            return m
        }
        lock.unlock()
        var m: [String: (id: String, raw: String)] = [:]
        var used: Set<String> = []
        for (offset, asset) in assets.enumerated() {
            let raw = PhotoAssetNaming.displayName(
                filename: asset.filename,
                creationDateMs: asset.creationDateMs,
                index: offset + 1)
            let unique = PhotoAssetNaming.uniqueName(raw, used: &used)
            m[unique] = (asset.id, raw)
        }
        lock.lock()
        cache[album] = (Date(), m)
        lock.unlock()
        return m
    }
}

/// 缩略图请求宽钳制上界(daemon `preview::thumb::THUMB_MAX_WIDTH` 同值;
/// 降采样参数面双侧同口径——旧 daemon 透传防御,巨 w 的解码位图分配是
/// 内存炸弹面)。
public let thumbMaxWidth: Int = 1024

/// 缩略图生成面(实机 = App 目标 IOSThumbEncoder;宿主单测注入伪编码器
/// ——macOS 宿主不可触 AVAssetImageGenerator/UIImage 真解码,解码正确性
/// 归实机冒烟)。生成 ≤w 宽 JPEG;失败/不可解 → nil(拒答,引擎落既有
/// 回退)。
public protocol ThumbEncoder: Sendable {
    /// [fd] 的只读源(**fd 所有权归调用方**——编码器不关不消费;阻塞解
    /// 码面由实现方自派后台,await 不占调用方隔离域)。
    func jpeg(fd: Int32, displayName: String, w: Int) async -> Data?
}

/// 缩略图桥应答器:构造后随事件流 run(单消费)。
public struct ThumbResponder: Sendable {

    /// 照片库数据源(App = PHPhotoLibraryService)。
    public let service: any PhotoLibraryService
    /// fdpass thumb 通道(0x0B 应答——daemon 侧 ThumbBridge 单 request_id
    /// 计数器,与源/查询/照片/服务/seek 通道空间各自独立)。
    public let fdpass: any FdPassAnswering
    /// 文件臂句柄面(授权根沙箱真实路径;与 SeekResponder 共用同一抽象)。
    public let files: any SeekOpeningFiles
    /// 生成件临时目录锚 = `{home}/thumb-serve`(``EngineLayout/
    /// thumbServeDirectory``;与 daemon preview::thumb 的 thumb_serve_dir
    /// 同锚,两侧不得漂移)。
    public let thumbDir: String
    /// 缩略图编码面(生产 = App 目标 IOSThumbEncoder)。
    public let encoder: any ThumbEncoder
    public let nameIndex = PhotoNameIndex()

    public init(
        service: any PhotoLibraryService,
        fdpass: any FdPassAnswering,
        files: any SeekOpeningFiles = DefaultSeekFiles(),
        thumbDir: String,
        encoder: any ThumbEncoder,
    ) {
        self.service = service
        self.fdpass = fdpass
        self.files = files
        self.thumbDir = thumbDir
        self.encoder = encoder
    }

    /// 事件流消费主环:挂载即清扫,然后 fs.request_thumb 应答,其余事件
    /// 直通忽略(各屏订阅面互不抢流)。field r26g #4:并发 3→5(吞吐
    /// ×1.7;硬件 HEIC 解码器共享资源上界 ~6,5 = 安全余量)。
    public func run(_ events: AsyncStream<IpcEvent>) async {
        sweep()
        let gate = ConcurrentGate(limit: 5)
        for await event in events {
            await gate.wait()
            Task {
                await handle(event)
                await gate.signal()
            }
        }
        // 事件流终结后等在飞应答归还(不弃 0x0B 交付;run 返回即全量定
        // 局——测试/关停面确定性)
        await gate.drain()
    }

    /// 单事件处置(测试直驱面)。
    public func handle(_ event: IpcEvent) async {
        guard case .requestThumb(let requestId, let path, let w) = event else {
            return
        }
        await answerThumb(requestId: requestId, path: path, w: w)
    }

    // MARK: 应答主线

    /// fs.request_thumb → 快径先行 → 开源 → 编码 → 0x0B 交付。w 钳制在此
    /// 单点(daemon 引擎面已钳 1..1024,旧 daemon 透传防御);拒答词面
    /// 同旧(引擎落既有回退,不回错误帧)。
    /// field r26g #1(顺序反转):照片臂快径**前置**——纯解析(名→
    /// photoId,O(1) 字典 + 15s TTL)后直接 PHImageManager 系统缩略图,
    /// **零 openSeekable**(此前 openThumbSource 无条件先跑
    /// requestContentEditingInput 全尺寸渲染,90%+ 场景成本白付——冷缩略
    /// 图慢 + 手机卡的最大剩余根因);快径不中(罕见)落下方全量路,其内
    /// 部二次解析走缓存,成本可忽略。文件臂无快径,直落全量路。
    func answerThumb(requestId: UInt64, path: String, w: UInt64) async {
        let width = min(max(Int(clamping: w), 1), thumbMaxWidth)
        // 照片臂快径先行(r26g #1):解析 → PHImageManager → 内联交付——
        // 不开原文件
        if let sub = PhotoVpath.sub(path),
           let resolved = await resolvePhoto(sub: sub),
           let fast = await service.thumbData(photoId: resolved.photoId, w: width),
           !fast.isEmpty {
            await deliver(requestId: requestId, displayName: resolved.name, bytes: fast)
            return
        }
        // 全量路(照片臂快径不中回退 / 文件臂):开源 → 编码
        let source: ThumbSource
        switch await openThumbSource(path) {
        case .success(let s):
            source = s
        case .failure(let e):
            await refuse(requestId: requestId, reason: e.reason)
            return
        }
        // 解码窗即源生命周期:编码后关 fd(生成失败面同样关闭,不泄漏——
        // defer 覆盖编码与交付两段)
        defer { Darwin.close(source.fd) }
        let bytes = await encoder.jpeg(fd: source.fd, displayName: source.displayName, w: width)
        guard let bytes = bytes, !bytes.isEmpty else {
            await refuse(requestId: requestId, reason: "无法生成缩略图: \(source.displayName)")
            return
        }
        await deliver(requestId: requestId, displayName: source.displayName, bytes: bytes)
    }

    /// 开源结果:只读 fd + 显示名(编码器视频/图片分型凭据;Android
    /// ThumbSource 同构。field r26g #1:photoId 凭据随快径前置撤——解析
    /// 在 answerThumb 快径面先行,开源面不再携带)。
    struct ThumbSource {
        let fd: Int32
        let displayName: String
    }

    /// 开源失败词面(Result 的 Failure 面;String 不 conform Error——薄
    /// 包装,[reason] = 0x0B 拒答 error 词面)。
    struct ThumbSourceError: Error {
        let reason: String
    }

    /// vpath → 源(可 seek 只读 fd;缩略图解码只读面与 seek 同原语)。
    /// `/照片图库` 前缀 = 照片臂,其余 = 授权根沙箱真实路径面;根自身/
    /// 目录/缺席/未授权一律失败词面(拒答 → 引擎回退)。
    func openThumbSource(_ path: String) async -> Result<ThumbSource, ThumbSourceError> {
        if let sub = PhotoVpath.sub(path) {
            return await openPhotoSource(sub: sub)
        }
        return openFileSource(path)
    }

    // MARK: 照片臂(0x06 清单同序唯一名重放)

    /// 照片臂纯解析(快径前置面;field r26g #1):`相册/名` →
    /// `(photoId, name)`——零 openSeekable/零 requestContentEditingInput
    /// (albums/assets 走 15s TTL 缓存 + O(1) 名字典)。任一步缺席 = nil,
    /// 调用方落全量路拿既有拒答词面(全量路二次解析走缓存,零重复成本)。
    func resolvePhoto(sub: String) async -> (photoId: String, name: String)? {
        guard await isAuthorized(), !sub.isEmpty else { return nil }
        let album: String
        let name: String
        if let slash = sub.firstIndex(of: "/") {
            album = String(sub[sub.startIndex..<slash])
            name = String(sub[sub.index(after: slash)...])
        } else {
            album = sub
            name = ""
        }
        guard !name.isEmpty else { return nil }
        guard let entry = try? await service.albums().first(where: { $0.name == album }),
              let assets = try? await service.assets(albumId: entry.isRecents ? nil : entry.id)
        else { return nil }
        guard let hit = nameIndex.map(for: album, assets: assets)[name] else { return nil }
        return (photoId: hit.id, name: name)
    }

    /// 照片源:`相册/名` → 批内唯一名重放(PhotoResponder 同序同函数)
    /// → `openSeekable`;根("")/无文件段/相册缺席/名缺席/资产不可开 =
    /// 失败词面(iCloud 未卸载 = 稳定词 `icloud_not_local` 透传)。
    func openPhotoSource(sub: String) async -> Result<ThumbSource, ThumbSourceError> {
        guard await isAuthorized() else {
            return .failure(ThumbSourceError(reason: "照片权限未授予"))
        }
        if sub.isEmpty {
            return .failure(ThumbSourceError(reason: "照片图库根不可出图(仅文件)"))
        }
        // 相册/名 切分(Kotlin substringBefore/After 同口径,SeekResponder
        // 同面)
        let album: String
        let name: String
        if let slash = sub.firstIndex(of: "/") {
            album = String(sub[sub.startIndex..<slash])
            name = String(sub[sub.index(after: slash)...])
        } else {
            album = sub
            name = ""
        }
        if name.isEmpty {
            return .failure(ThumbSourceError(reason: "相册不可出图(仅文件): \(album)"))
        }
        do {
            guard let entry = try await service.albums().first(where: { $0.name == album }) else {
                return .failure(ThumbSourceError(reason: "无此相册: \(album)"))
            }
            let assets = try await service.assets(albumId: entry.isRecents ? nil : entry.id)
            // field r26f fix2: O(1) dict lookup replaces O(n) replay.
            // field r26g 补:索引值携 raw 原始名(去 ` (n)` 后缀——
            // Android ThumbSource.displayName = asset.name 同面)。
            guard let hit = nameIndex.map(for: album, assets: assets)[name] else {
                return .failure(ThumbSourceError(reason: "源已不存在: \(album)/\(name)"))
            }
            let fd = try await service.openSeekable(photoId: hit.id)
            return .success(ThumbSource(fd: fd, displayName: hit.raw))
        } catch let e as SeekOpenError {
            return .failure(ThumbSourceError(reason: e.word))
        } catch {
            return .failure(ThumbSourceError(reason: "\(error)"))
        }
    }

    /// 授权可答(authorized/limited;limited = 子集内照常)。
    func isAuthorized() async -> Bool {
        let status = await service.permissionStatus()
        return status == .authorized || status == .limited
    }

    // MARK: 文件臂(授权根沙箱真实路径)

    /// 文件源:绝对路径 → open(2) 只读 fd(``DefaultSeekFiles``;缺席/
    /// 目录/不可读 = 失败词面——目录 fail-closed「不可出图」)。
    func openFileSource(_ path: String) -> Result<ThumbSource, ThumbSourceError> {
        do {
            let fd = try files.open(path: path)
            let name = (path as NSString).lastPathComponent
            return .success(ThumbSource(fd: fd, displayName: name))
        } catch {
            return .failure(ThumbSourceError(reason: "源不可出图: \(error)"))
        }
    }

    // MARK: 落盘与帧化

    /// field r26f 修3(现役形态):0x0B 内联 JPEG——base64 直置 JSON
    /// `{jpeg_b64, ok, request_id}`(帧帽 8MiB 下缩略图恒适配;撤临时
    /// 文件三跳「写→引擎读→删」,I/O 面零文件)。
    func deliver(requestId: UInt64, displayName: String, bytes: Data) async {
        let payload: [String: Any] = [
            "ok": true,
            "jpeg_b64": bytes.base64EncodedString(),
            "request_id": Int(clamping: requestId),
        ]
        _ = try? await fdpass.send(kind: FdPassKind.thumbData, json: payload)
    }

    /// ok:false 拒答(引擎按「桥不可用」落既有回退——缩略图缺位只是降级,
    /// 词面仅日志消费;iCloud 形 = 稳定词 icloud_not_local 透传)。
    func refuse(requestId: UInt64, reason: String) async {
        _ = try? await fdpass.sendThumbData(requestId: requestId, path: nil, error: reason)
    }

    /// 挂载清扫:thumb-serve 整目录清空(残件自愈;目录本体保留)。与引擎
    /// 缓存目录 thumb-tmp 分立——本清扫若同目录会把引擎 LRU 缓存抹掉
    /// (daemon preview::thumb.rs 模块注钉死;PhotoResponder 挂载清扫同款)。
    public func sweep() {
        let fm = FileManager.default
        try? fm.createDirectory(atPath: thumbDir, withIntermediateDirectories: true)
        guard let items = try? fm.contentsOfDirectory(atPath: thumbDir) else { return }
        for item in items {
            try? fm.removeItem(atPath: thumbDir + "/" + item)
        }
    }
}
