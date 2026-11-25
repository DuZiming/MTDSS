/// 转码桥应答器(V4-1 T14 预览波;daemon 侧 jobs.rs RenditionBridge /
/// fdpass 0x0C 契约为真源,Android `serve/RenditionResponder.kt` 同构):
/// 消费 daemon 的 `fs.rendition_open` / `fs.rendition_cancel` 事件——源
/// 解析(``PhotoVpath`` 共享单点,照片臂/文件臂两臂与 Seek/Thumb 同面)
/// → 会话目录 `{home}/rendition-serve/{session}/` → **回执先行**(daemon
/// open 等待窗只盖源解析;真管线启动在回执之后异步,慢启动不烧等待窗)→
/// 管线逐段产出落盘 + 0x0C `{index, ok, path, request_id}` 段回告(daemon
/// 按需读已产段,T12「读已产段」语义);失败 = 0x0C `{error, ok:false}`
/// (daemon 落 unsupported → 查看端回退原流)。
///
/// - **会话键**:session = open 的 request_id(daemon 单调源;cancel/段回告
///   按它路由;本地目录名即会话键,daemon 读面按会话目录前缀校验 fail-closed)。
/// - **start_at**:起点毫秒透传管线(seek = 「起点段」重开会话的原料;
///   就近关键帧落地在管线内——Android SEEK_TO_PREVIOUS_SYNC 同语义)。
/// - **cancel**:置取消旗 → 删段目录 → 回 `{ok, request_id}`;未知 session
///   幂等照成(daemon cancel 同语义;残件自愈面)。管线在下个轮询点退出,
///   写盘竞窗的孤儿件由挂载清扫兜底。
/// - **生成件生命周期**:段文件归 App 管——cancel 即清;管线失败**不删**
///   (已产段 daemon 照读可播,失败标记只对未产段生效);挂载清扫整目录清
///   (daemon 只读不删,thumb-serve 三面同构)。
/// - **转码面**:``RenditionTranscoding`` 注入缝(宿主单测注入伪管线
///   ——macOS 宿主不可触 AVAssetReader/VTCompressionSession 真解码,
///   解码正确性归实机冒烟;生产 = App 目标 IOSRenditionTranscoder,
///   Ruling E 同款绑定归 App)。fd 所有权随入参交出管线(终局必关);
///   回执未受理 = 不开管线即收源清目录(白烧电防线)。
/// - **生命周期**:App boot 时随 EventHub 订阅流 `run` 起来(挂载即清扫;
///   事件流终结即退,与引擎同寿;Photo/Seek/Thumb 应答器同款)。畸形事件
///   帧在解析面丢弃(类型化事件无缺字段形态);应答帧 ack ok:false =
///   请求已超时/未知,静默收口(等待面已摘,事实以引擎侧读面为准)。

import Foundation

/// 转码管线面(实机 = App 目标 IOSRenditionTranscoder;宿主单测注入伪
/// 管线——macOS 宿主不可触 AVAssetReader/VTCompressionSession 真解码,
/// 解码/编码正确性归实机冒烟)。实现契约(Android TranscoderFactory 同构):
/// - **fd 所有权随入参交出**:终局(EOF/失败/取消)必关;
/// - 段文件落 `sessionDir` 后经 `notifier` 回告(index 0 = init 段,
///   T12 段序约定);阻塞/重活面由实现方自派后台,await 不占调用方隔离域;
/// - EOF/取消 = 正常返回;失败 = 抛错(响应器统一转 0x0C ok:false 回告);
/// - `isCancelled` 逐循环轮询(取消失能后 ≤ 单轮询点内退出)。
public protocol RenditionTranscoding: Sendable {
    func transcode(
        fd: Int32,
        displayName: String,
        startAtMs: Int64,
        sessionDir: String,
        notifier: @escaping @Sendable (_ index: Int, _ path: String) -> Void,
        isCancelled: @escaping @Sendable () -> Bool,
    ) async throws
}

/// 转码会话取消旗(管线内部退出信号;响应器按旗静默)。
final class RenditionCancelFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var value = false

    func set() {
        lock.lock(); value = true; lock.unlock()
    }

    func get() -> Bool {
        lock.lock(); defer { lock.unlock() }
        return value
    }
}

/// 在册会话表(session → 取消旗;cancel/终局摘表)。
final class RenditionSessionTable: @unchecked Sendable {
    private let lock = NSLock()
    private var flags: [UInt64: RenditionCancelFlag] = [:]

    func register(_ session: UInt64) -> RenditionCancelFlag {
        let flag = RenditionCancelFlag()
        lock.lock(); flags[session] = flag; lock.unlock()
        return flag
    }

    /// 摘表并置旗(在册 = true;未知会话 = false 幂等面)。
    @discardableResult
    func removeAndCancel(_ session: UInt64) -> Bool {
        lock.lock()
        let flag = flags.removeValue(forKey: session)
        lock.unlock()
        flag?.set()
        return flag != nil
    }
}

/// 源总时长探针(r29;App 注入面 = F_GETPATH → AVURLAsset.load(.duration)):
/// 回执携 duration_ms → 引擎查看端 playlist 全长一次列 + ENDLIST(VOD 形,
/// 时长自第一帧即全长);0 = 未知(缺省注入 = EVENT 生长形)。
public protocol RenditionDurationProbing: Sendable {
    func durationMs(fd: Int32, displayName: String) async -> UInt64
}

/// 段回告在飞计数闩(r29):notifier 的无序 Task 发送 begin/end 记账,
/// eof 发送前 idle() 等清零——保「eof 最后落帧」wire 序。
final class SegmentSendLatch: @unchecked Sendable {
    private let lock = NSLock()
    private var inFlight = 0

    func begin() {
        lock.lock(); inFlight += 1; lock.unlock()
    }

    func end() {
        lock.lock(); inFlight -= 1; lock.unlock()
    }

    func idle() async {
        while true {
            lock.lock()
            let busy = inFlight
            lock.unlock()
            if busy == 0 { return }
            try? await Task.sleep(nanoseconds: 5_000_000)
        }
    }
}

/// 转码桥应答器:构造后随事件流 run(单消费)。
public struct RenditionResponder: Sendable {

    /// 照片库数据源(App = PHPhotoLibraryService)。
    public let service: any PhotoLibraryService
    /// fdpass rendition 通道(0x0C 应答——daemon 侧 RenditionBridge 单
    /// request_id 计数器,与源/查询/照片/服务/seek/thumb 通道空间各自独立)。
    public let fdpass: any FdPassAnswering
    /// 文件臂句柄面(授权根沙箱真实路径;与 Seek/Thumb 共用同一抽象)。
    public let files: any SeekOpeningFiles
    /// 段文件目录锚 = `{home}/rendition-serve`(``EngineLayout/
    /// renditionServeDirectory``;与 daemon preview::rendition 的
    /// rendition_serve_dir 同锚,两侧不得漂移)。
    public let renditionDir: String
    /// 转码管线面(生产 = App 目标 IOSRenditionTranscoder)。
    public let transcoder: any RenditionTranscoding
    /// 源总时长探针(r29;nil = 0 未知,EVENT 生长形)。
    public let durationProbe: (any RenditionDurationProbing)?

    private let sessions = RenditionSessionTable()

    public init(
        service: any PhotoLibraryService,
        fdpass: any FdPassAnswering,
        files: any SeekOpeningFiles = DefaultSeekFiles(),
        renditionDir: String,
        transcoder: any RenditionTranscoding,
        durationProbe: (any RenditionDurationProbing)? = nil,
    ) {
        self.service = service
        self.fdpass = fdpass
        self.files = files
        self.renditionDir = renditionDir
        self.transcoder = transcoder
        self.durationProbe = durationProbe
    }

    /// 事件流消费主环:挂载即清扫,然后 fs.rendition_* 应答,其余事件
    /// 直通忽略(各屏订阅面互不抢流)。
    public func run(_ events: AsyncStream<IpcEvent>) async {
        sweep()
        for await event in events {
            await handle(event)
        }
    }

    /// 单事件处置(测试直驱面)。
    public func handle(_ event: IpcEvent) async {
        switch event {
        case .renditionOpen(let requestId, let path, let startAtMs):
            await answerOpen(requestId: requestId, path: path, startAtMs: startAtMs)
        case .renditionCancel(let requestId, let session):
            await answerCancel(requestId: requestId, session: session)
        default:
            return
        }
    }

    // MARK: open(fs.rendition_open)

    /// 开源结果:只读 fd + 显示名(管线命名/分型凭据;Android OpenedSource
    /// + displayName 同构)。fd 在受理后随入参交出管线,其余路径本侧关。
    struct RenditionSource {
        let fd: Int32
        let displayName: String
    }

    /// 开源失败词面(Result 的 Failure 面;String 不 conform Error——薄
    /// 包装,[reason] = 0x0C 拒答 error 词面)。
    struct RenditionSourceError: Error {
        let reason: String
    }

    /// fs.rendition_open → 开源 → 会话目录 → 回执先行 → 管线异步起。
    /// 开源失败/目录创建失败一律拒答词面(daemon 落 unsupported → 查看端
    /// 回退原流+海报;词面仅日志消费)。
    func answerOpen(requestId: UInt64, path: String, startAtMs: UInt64) async {
        // 源解析(照片/文件两臂):失败 = 拒答
        let source: RenditionSource
        switch await openSource(path) {
        case .success(let s):
            source = s
        case .failure(let e):
            await refuse(requestId: requestId, reason: e.reason)
            return
        }
        // 会话目录(目录名 = 会话键;daemon 读面按此目录前缀校验)
        let sessionDir = "\(renditionDir)/\(requestId)"
        var isDir: ObjCBool = false
        let dirOk =
            FileManager.default.fileExists(atPath: sessionDir, isDirectory: &isDir)
            && isDir.boolValue
            || (try? FileManager.default.createDirectory(
                atPath: sessionDir, withIntermediateDirectories: true)) != nil
        if !dirOk {
            Darwin.close(source.fd)
            await refuse(requestId: requestId, reason: "会话目录创建失败: \(sessionDir)")
            return
        }
        // 在册(段回告/取消旗路由)+ 回执先行(管线受理;真管线启动在
        // 回执后异步)。r29:回执携源总时长(探针;0 = 未知)
        let durationMs = await durationProbe?.durationMs(
            fd: source.fd, displayName: source.displayName) ?? 0
        let cancelled = sessions.register(requestId)
        let accepted = await ackAccepted(requestId: requestId, durationMs: durationMs)
        if !accepted {
            // daemon 未受理(已超时/帧丢失):不开管线(白烧电),即收
            sessions.removeAndCancel(requestId)
            Darwin.close(source.fd)
            try? FileManager.default.removeItem(atPath: sessionDir)
            return
        }
        // 管线异步起(段回告逐段 Task 直发;失败统一回告;取消静默)。
        // r29:段回告/eof 发送闩——eof 前**等在飞段回告清零**(段回告是
        // 无序 Task,eof 内联发送会抢先落帧破坏「eof 最后」契约;引擎侧
        // 容忍乱序,本闩保 wire 序 + 测试确定性)
        let latch = SegmentSendLatch()
        let fdpass = self.fdpass
        let transcoder = self.transcoder
        let table = sessions
        Task.detached(priority: .userInitiated) {
            do {
                try await transcoder.transcode(
                    fd: source.fd,
                    displayName: source.displayName,
                    startAtMs: Int64(clamping: startAtMs),
                    sessionDir: sessionDir,
                    notifier: { index, path in
                        if cancelled.get() { return }
                        latch.begin()
                        Task {
                            defer { latch.end() }
                            _ = try? await fdpass.sendRenditionSegment(
                                requestId: requestId, index: index, path: path)
                        }
                    },
                    isCancelled: { cancelled.get() },
                )
                // field 修(0x0C eof 形):管线自然完毕通知——引擎记会话完
                // 毕(越界段答段越界),查看端 playlist 封 ENDLIST(时长锁
                ///全长 + 播放端结束态);取消面静默;先等段回告在飞清零
                await latch.idle()
                if !cancelled.get() {
                    _ = try? await fdpass.sendRenditionEof(requestId: requestId)
                }
            } catch {
                // 失败回告(取消面静默——旗在管线退出前置位即无回告噪音)
                if !cancelled.get() {
                    let word = (error as? TranscodeFailure)?.message ?? "\(error)"
                    _ = try? await fdpass.sendRenditionRefuse(
                        requestId: requestId, error: "转码管线失败: \(word)")
                }
            }
            table.removeAndCancel(requestId)
        }
    }

    /// vpath → 源(可 seek 只读 fd;转码只读面与 seek/thumb 同原语)。
    /// `/照片图库` 前缀 = 照片臂,其余 = 授权根沙箱真实路径面;根自身/
    /// 目录/缺席/未授权一律失败词面(拒答 → 引擎回退)。
    func openSource(_ path: String) async -> Result<RenditionSource, RenditionSourceError> {
        if let sub = PhotoVpath.sub(path) {
            return await openPhotoSource(sub: sub)
        }
        return openFileSource(path)
    }

    // MARK: 照片臂(0x06 清单同序唯一名重放)

    /// 照片源:`相册/名` → 批内唯一名重放(PhotoResponder 同序同函数)
    /// → `openSeekable`;根("")/无文件段/相册缺席/名缺席/资产不可开 =
    /// 失败词面(iCloud 未卸载 = 稳定词 `icloud_not_local` 透传)。**无
    /// size 守卫**——转码吃「解码窗即源生命周期」的可渲染源,编辑件渲染
    /// 面出片即所见(thumb 同口径;seek 的原件字节面守卫在此不适用)。
    func openPhotoSource(sub: String) async -> Result<RenditionSource, RenditionSourceError> {
        guard await isAuthorized() else {
            return .failure(RenditionSourceError(reason: "照片权限未授予"))
        }
        if sub.isEmpty {
            return .failure(RenditionSourceError(reason: "照片图库根不可转码(仅文件)"))
        }
        // 相册/名 切分(Kotlin substringBefore/After 同口径,Seek/Thumb 同面)
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
            return .failure(RenditionSourceError(reason: "相册不可转码(仅文件): \(album)"))
        }
        do {
            guard let entry = try await service.albums().first(where: { $0.name == album }) else {
                return .failure(RenditionSourceError(reason: "无此相册: \(album)"))
            }
            let assets = try await service.assets(albumId: entry.isRecents ? nil : entry.id)
            // 批内唯一名重放(listEntries 同序同函数——0x06 面逐条唯一;
            // 显示名 = 原始名——分型/生成件命名凭据,Android 同面)
            var used: Set<String> = []
            for (offset, asset) in assets.enumerated() {
                let raw = PhotoAssetNaming.displayName(
                    filename: asset.filename, creationDateMs: asset.creationDateMs,
                    index: offset + 1)
                if PhotoAssetNaming.uniqueName(raw, used: &used) == name {
                    let fd = try await service.openSeekable(photoId: asset.id)
                    return .success(RenditionSource(fd: fd, displayName: raw))
                }
            }
            return .failure(RenditionSourceError(reason: "源已不存在: \(album)/\(name)"))
        } catch let e as SeekOpenError {
            return .failure(RenditionSourceError(reason: e.word))
        } catch {
            return .failure(RenditionSourceError(reason: "\(error)"))
        }
    }

    /// 授权可答(authorized/limited;limited = 子集内照常)。
    func isAuthorized() async -> Bool {
        let status = await service.permissionStatus()
        return status == .authorized || status == .limited
    }

    // MARK: 文件臂(授权根沙箱真实路径)

    /// 文件源:绝对路径 → open(2) 只读 fd(``DefaultSeekFiles``;缺席/
    /// 目录/不可读 = 失败词面——目录 fail-closed「不可转码」)。
    func openFileSource(_ path: String) -> Result<RenditionSource, RenditionSourceError> {
        var isDir: ObjCBool = false
        if FileManager.default.fileExists(atPath: path, isDirectory: &isDir), isDir.boolValue {
            return .failure(RenditionSourceError(reason: "目录不可转码(仅文件): \(path)"))
        }
        do {
            let fd = try files.open(path: path)
            let name = (path as NSString).lastPathComponent
            return .success(RenditionSource(fd: fd, displayName: name))
        } catch {
            return .failure(RenditionSourceError(reason: "源不可开: \(error)"))
        }
    }

    // MARK: 帧化

    /// ok:false 拒答(daemon 落 unsupported → 查看端回退原流;词面仅日志
    /// 消费,iCloud 形 = 稳定词 icloud_not_local 透传)。应答帧 ack ok:false
    /// = 请求已超时/未知,静默收口(等待面已摘,事实以引擎侧读面为准)。
    func refuse(requestId: UInt64, reason: String) async {
        _ = try? await fdpass.sendRenditionRefuse(requestId: requestId, error: reason)
    }

    // MARK: cancel(fs.rendition_cancel)

    /// 回执受理判读(0x81 ack payload 的 ok 键;false/不可解 = 未受理)。
    private func ackAccepted(requestId: UInt64, durationMs: UInt64 = 0) async -> Bool {
        guard let ack = try? await fdpass.sendRenditionAck(
            requestId: requestId, durationMs: durationMs) else {
            return false
        }
        let json = (try? JSONSerialization.jsonObject(with: ack.payload)) as? [String: Any]
        return (json?["ok"] as? Bool) == true
    }

    /// cancel:置旗 → 清段目录 → 回执(未知会话幂等照成;daemon cancel
    /// 发后即忘,回执按未知 id 拒——发送失败静默)。管线在下个轮询点退出;
    /// 写盘竞窗孤儿件由挂载清扫兜底。
    func answerCancel(requestId: UInt64, session: UInt64) async {
        sessions.removeAndCancel(session)
        let dir = "\(renditionDir)/\(session)"
        if FileManager.default.fileExists(atPath: dir) {
            try? FileManager.default.removeItem(atPath: dir)
        }
        _ = try? await fdpass.sendRenditionAck(requestId: requestId)
    }

    // MARK: 挂载清扫

    /// 挂载清扫:rendition-serve 整目录清空(残件自愈;目录本体保留)。与
    /// 引擎缓存目录分立(thumb-serve 同哲学——清扫整目录清空,同目录会把
    /// 引擎 LRU 缓存抹掉;PhotoResponder 挂载清扫同款)。
    public func sweep() {
        let fm = FileManager.default
        try? fm.createDirectory(atPath: renditionDir, withIntermediateDirectories: true)
        guard let items = try? fm.contentsOfDirectory(atPath: renditionDir) else { return }
        for item in items {
            try? fm.removeItem(atPath: renditionDir + "/" + item)
        }
    }
}
