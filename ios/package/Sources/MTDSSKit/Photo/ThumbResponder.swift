/// 缩略图桥应答器(V4-1 T11 预览波;daemon 侧 jobs.rs ThumbBridge /
/// fdpass 0x0B 契约为真源,Android ThumbResponder.kt 同构):消费 daemon
/// 的 `fs.request_thumb {request_id, path, w}` 事件,按 path 分流两源开
/// fd,经 ``ThumbEncoder`` 生成 ≤w 宽 JPEG 字节落自有临时目录
/// `{home}/thumb-serve/`,经 fdpass **thumb 通道**(0x0B,单实例)回
/// `{ok, path, request_id}`(字母序)——引擎读字节入源端缓存后 unlink
/// (缩略图缺位只是降级:拒答时引擎落既有回退,不回错误帧)。
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
/// - **生成件生命周期**:落盘 `{requestId}-{名}.jpg`(请求级唯一名),引擎
///   读毕 unlink,**不删**(ack 只证明帧受理,不证明字节已读)——**挂载
///   清扫**清空 thumb-serve 兜底残件(目录与引擎缓存目录 thumb-tmp 分立
///   ——清扫整目录清空,同目录会抹掉引擎 LRU 缓存,daemon
///   preview::thumb.rs 模块注钉死;Android ThumbResponder 同纪律)。
/// - **编码面**:``ThumbEncoder`` 注入缝(宿主单测注入伪编码器——macOS
///   宿主不可触 AVAssetImageGenerator/UIImage 真解码,解码正确性归实机
///   冒烟;生产 = App 目标 IOSThumbEncoder,Ruling E 同款绑定归 App)。
/// - **生命周期**:App boot 时随 EventHub 订阅流 `run` 起来(事件流终结
///   即退,与引擎同寿;PhotoResponder 同款——挂载即清扫)。畸形事件帧在
///   解析面丢弃(类型化事件无缺字段形态);应答帧 ack ok:false = 请求已
///   超时/未知,静默收口(等待面已摘,事实以引擎侧读面为准)。

import Foundation

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
    /// 直通忽略(各屏订阅面互不抢流)。
    public func run(_ events: AsyncStream<IpcEvent>) async {
        sweep()
        for await event in events {
            await handle(event)
        }
    }

    /// 单事件处置(测试直驱面)。
    public func handle(_ event: IpcEvent) async {
        guard case .requestThumb(let requestId, let path, let w) = event else {
            return
        }
        await answerThumb(requestId: requestId, path: path, w: w)
    }

    // MARK: 应答主线

    /// fs.request_thumb → 开源 → 编码 → 落盘 → 0x0B 交付。w 钳制在此单点
    /// (daemon 引擎面已钳 1..1024,旧 daemon 透传防御);开源失败/编码
    /// 失败/落盘失败一律拒答词面(引擎落既有回退,不回错误帧)。
    func answerThumb(requestId: UInt64, path: String, w: UInt64) async {
        let width = min(max(Int(clamping: w), 1), thumbMaxWidth)
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

    /// 开源结果:只读 fd + 显示名(编码器视频/图片分型凭据 + 生成件命名;
    /// Android ThumbSource 同构)。
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
            // 批内唯一名重放(listEntries 同序同函数——0x06 面逐条唯一);
            // 显示名 = 原始名(Android ThumbSource.displayName = asset.name
            // 同面——重放比对用唯一名,生成件命名/分型凭据用原始名)
            var used: Set<String> = []
            for (offset, asset) in assets.enumerated() {
                let raw = PhotoAssetNaming.displayName(
                    filename: asset.filename, creationDateMs: asset.creationDateMs,
                    index: offset + 1)
                if PhotoAssetNaming.uniqueName(raw, used: &used) == name {
                    let fd = try await service.openSeekable(photoId: asset.id)
                    return .success(ThumbSource(fd: fd, displayName: raw))
                }
            }
            return .failure(ThumbSourceError(reason: "源已不存在: \(album)/\(name)"))
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

    /// 编码产物落盘 → 0x0B `{ok, path, request_id}` → 0x81 ack 回执。**不删**
    /// 生成件(引擎异步读——ack 只证明帧受理;引擎读毕 unlink,本侧挂载
    /// 清扫兜底,serve-tmp/photo-serve 同哲学)。
    func deliver(requestId: UInt64, displayName: String, bytes: Data) async {
        try? FileManager.default.createDirectory(atPath: thumbDir, withIntermediateDirectories: true)
        // 显示名单段化(资产/授权根名不含分隔符,防御性替换)
        let dest = "\(thumbDir)/\(requestId)-\(displayName.replacingOccurrences(of: "/", with: "_")).jpg"
        do {
            try bytes.write(to: URL(fileURLWithPath: dest))
        } catch {
            await refuse(requestId: requestId, reason: "缩略图落盘失败: \(error)")
            return
        }
        do {
            _ = try await fdpass.sendThumbData(requestId: requestId, path: dest)
        } catch {
            // 发送失败:连接断/超窗——生成件留 thumb-serve(挂载清扫兜底);
            // 引擎等待面超时即摘落回退,迟到无 ack 无残留(SeekResponder
            // deliver 静默收口同面)。
        }
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
