/// ShareQueue — 分享扩展的转交队列与主 App 消费面(T11,spec §4.7/§5.4):
///
/// **形态**:App Group 容器 `{root}/queue/<item>/` 为队列项(item 目录),
/// `files/` 下两步拷贝(`.part` 临时名 → 同目录 rename 完成),`meta.json`
/// 记意图(目标设备 + 声明尺寸 + 状态)。扩展秒级寿命、主 App 回前台消费,
/// 两端只经本面读写(Codable 蛇形键 golden 化,ShareQueueTests 钉死)。
///
/// **两步拷贝与失败语义(评审 #4 裁定:数据丢失形状的意外不可接受)**:
/// - meta 先行(status=.copying,声明逐文件 size)→ 逐文件 `.part` →
///   rename → 全部落定翻 .ready。扩展中途被杀:.copying 项在宽限窗
///   (`defaultCopyGrace`,防主 App 消费撞上仍在跑的拷贝)外判
///   「拷贝中断」失败态;孤儿目录(无 meta)同判。
/// - 消费 = scan → 校验(实际 size 对声明,不符/缺件拒发)→ 文件两步
///   move 进收件箱(撞名 ` (n)` 顺延,InboxPublisher 同语言)→ 发送
///   (send 闭包注入:App 侧接 SendService,Ruling I 收口)→ 成功清队列;
///   发送失败 = 文件留暂存(staged_paths 记录)+ failed 态上浮
///   「分享转交失败」可重试(重试走 staged 断点,不重拷)。
/// - **桥接形态(upload-bridge)不采用(裁定记录)**:契约的 source:"bridge"
///   需 App 侧 fd 提供机制(fdpass 0x01 SCM_RIGHTS 发送 + fs.request_source
///   监听),Swift 侧尚未存在;v1 以「暂存进收件箱 + 路径形态上传」这一
///   简报认可的极简等价实现(引擎源守卫 = 授权根收口,Documents 在授权集;
///   且路径形态上传失败后引擎 reinitiate 仍可用,bridge 形态必拒——T9
///   concern #3)。数据仅一拷贝落收件箱(Files App 可见,用户可寻回)。
///
/// 宿主可测:纯 Foundation,root 可注入(宿主测试用临时目录),send 闭包
/// 注入(不依赖活引擎)。

import Foundation

// MARK: - meta 模型(蛇形键,扩展↔主 App 两端)

/// 队列项内单文件声明(入队时刻的 name/size;消费时逐文件对实际校验)。
public struct ShareQueueFile: Codable, Equatable, Sendable {
    public var name: String
    public var size: Int64

    public init(name: String, size: Int64) {
        self.name = name
        self.size = size
    }
}

/// 队列项状态机:copying(扩展拷贝中)→ ready(待主 App 消费)→
/// staged(已暂存收件箱,发送未确认)→ 摘除(发送已受理)/ failed(可重试)。
public enum ShareQueueStatus: String, Codable, Sendable {
    case copying
    case ready
    case staged
    case failed
}

/// 队列项意图。键面蛇形(与简报 schema 逐字对齐:{target_device_id,
/// source_name, size, enqueued_at, status};files/staged_paths/error 为
/// 多文件与失败语义的补充面)。Date 编码 = epoch 秒(Double)。
public struct ShareQueueMeta: Codable, Equatable, Sendable {
    public var targetDeviceId: String
    public var sourceName: String
    public var size: Int64
    public var enqueuedAt: Date
    public var status: ShareQueueStatus
    public var files: [ShareQueueFile]
    /// 暂存收件箱后的绝对路径(staged/failed 态在场;重试不重拷)。
    public var stagedPaths: [String]?
    /// failed 态原因(「拷贝中断」「文件校验不符」或引擎错误原文)。
    public var error: String?

    public init(
        targetDeviceId: String, sourceName: String, size: Int64,
        enqueuedAt: Date, status: ShareQueueStatus, files: [ShareQueueFile],
        stagedPaths: [String]? = nil, error: String? = nil,
    ) {
        self.targetDeviceId = targetDeviceId
        self.sourceName = sourceName
        self.size = size
        self.enqueuedAt = enqueuedAt
        self.status = status
        self.files = files
        self.stagedPaths = stagedPaths
        self.error = error
    }

    enum CodingKeys: String, CodingKey {
        case targetDeviceId = "target_device_id"
        case sourceName = "source_name"
        case size
        case enqueuedAt = "enqueued_at"
        case status
        case files
        case stagedPaths = "staged_paths"
        case error
    }

    /// 解码容错:status 缺席(损坏)按 failed 收口——不静默丢。
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        targetDeviceId = try c.decode(String.self, forKey: .targetDeviceId)
        sourceName = try c.decode(String.self, forKey: .sourceName)
        size = try c.decode(Int64.self, forKey: .size)
        enqueuedAt = try c.decode(Date.self, forKey: .enqueuedAt)
        status = (try? c.decode(ShareQueueStatus.self, forKey: .status)) ?? .failed
        files = (try? c.decode([ShareQueueFile].self, forKey: .files)) ?? []
        stagedPaths = try? c.decode([String].self, forKey: .stagedPaths)
        error = try? c.decode(String.self, forKey: .error)
    }
}

/// scan 出口(id + meta;失败行上浮由消费方/呈现面分档)。
public struct ShareQueueEntry: Equatable, Sendable {
    public let id: String
    /// var:重试面的目标覆盖(retryOnce)在内存副本上改写后整面写回。
    public var meta: ShareQueueMeta

    public init(id: String, meta: ShareQueueMeta) {
        self.id = id
        self.meta = meta
    }
}

// MARK: - 队列存储(root 可注入)

public struct ShareQueueStore: Sendable {

    /// App Group 容器根(生产;宿主测试传临时目录)。
    public let root: URL

    /// 拷贝中断判定宽限窗(秒):meta=.copying 且入队早于此时点才判失败——
    /// 扩展 open() 拉起主 App 的路径上拷贝必已完成,窗口只防「用户手动
    /// 开 App 撞上仍在拷」的窄竞窗。
    public static let defaultCopyGrace: TimeInterval = 120

    public init(root: URL) {
        self.root = root
    }

    /// 生产容器根(App Group;模拟器/真机同 API)。未配置/不可得 = nil
    /// (扩展侧禁用入队,主 App 侧无队列)。
    public static func appGroupRoot(
        identifier: String = AppGroup.identifier, fileManager: FileManager = .default,
    ) -> URL? {
        fileManager.containerURL(forSecurityApplicationGroupIdentifier: identifier)
    }

    // MARK: 路径面

    /// 队列根 = `{group}/queue/`(简报口径;不在引擎 home 下——扩展无
    /// 引擎访问权,App Group 是唯一共享面)。
    public var queueDir: URL { root.appendingPathComponent("queue", isDirectory: true) }

    /// 单项目录 = `{queue}/<id>/`。
    public func itemDir(id: String) -> URL { queueDir.appendingPathComponent(id, isDirectory: true) }

    /// 单项文件目录 = `{queue}/<id>/files/`。
    public func filesDir(id: String) -> URL { itemDir(id: id).appendingPathComponent("files", isDirectory: true) }

    func metaURL(id: String) -> URL { itemDir(id: id).appendingPathComponent("meta.json") }

    /// 文件名净化(suggestedName 可能带路径分隔/盘符冒号;目录分量只取末段)。
    public static func sanitizedName(_ name: String) -> String {
        let stripped = name.split(separator: "/").last.map(String.init) ?? name
        return stripped
            .replacingOccurrences(of: ":", with: "_")
            .replacingOccurrences(of: "\\", with: "_")
    }

    // MARK: 写入面(扩展 + DEBUG 种子共用)

    /// 开项:建目录 + 写 meta(status=.copying,声明逐文件 size)。
    /// meta 原子写(临时名+rename),在场即可被消费端识别为「拷贝中」。
    public func beginItem(
        id: String, targetDeviceId: String, files: [ShareQueueFile], now: Date,
    ) throws {
        try fileManager.createDirectory(at: filesDir(id: id), withIntermediateDirectories: true)
        let sourceName = files.first.map(\.name).map(Self.sanitizedName) ?? ""
        let meta = ShareQueueMeta(
            targetDeviceId: targetDeviceId,
            sourceName: sourceName,
            size: files.reduce(0) { $0 + $1.size },
            enqueuedAt: now,
            status: .copying,
            files: files.map { ShareQueueFile(name: Self.sanitizedName($0.name), size: $0.size) },
        )
        try writeMeta(meta, id: id)
    }

    /// 单文件两步拷贝(`.part` 临时名 → 同目录 rename;P4a 语义——最终名
    /// 在场即完整)。源尺寸不匹配声明时拒拷(扩展侧早失败,不入坏项)。
    public func copyFile(item id: String, file: ShareQueueFile, from source: URL) throws {
        let name = Self.sanitizedName(file.name)
        let dest = filesDir(id: id).appendingPathComponent(name)
        let tmp = dest.appendingPathExtension("part")
        let attrs = try fileManager.attributesOfItem(atPath: source.path)
        let actual = (attrs[.size] as? NSNumber)?.int64Value ?? -1
        guard actual == file.size else {
            throw ShareQueueError.sizeMismatch(name: name, declared: file.size, actual: actual)
        }
        if fileManager.fileExists(atPath: tmp.path) {
            try fileManager.removeItem(at: tmp)
        }
        try fileManager.copyItem(at: source, to: tmp)
        if fileManager.fileExists(atPath: dest.path) {
            try fileManager.removeItem(at: dest)
        }
        try fileManager.moveItem(at: tmp, to: dest)
    }

    /// 收项:全部文件落定后翻 .ready(消费端放行)。
    public func finishItem(id: String, now: Date) throws {
        guard var meta = readMeta(id: id) else {
            throw ShareQueueError.metaMissing(id: id)
        }
        meta.status = .ready
        try writeMeta(meta, id: id)
    }

    // MARK: 读取/状态面(主 App)

    /// 读单项 meta(缺失/损坏 = nil)。
    public func readMeta(id: String) -> ShareQueueMeta? {
        guard let data = try? Data(contentsOf: metaURL(id: id)) else { return nil }
        return try? Self.metaDecoder.decode(ShareQueueMeta.self, from: data)
    }

    /// 扫描全队列(copying 宽限窗外与孤儿目录折算失败态并**持久化**,
    /// 失败行跨会话不丢)。序按 enqueuedAt 升序(先来先消费)。
    @discardableResult
    public func scan(
        now: Date, copyGrace: TimeInterval = ShareQueueStore.defaultCopyGrace,
        fileManager: FileManager = .default,
    ) throws -> [ShareQueueEntry] {
        let dirs = (try? fileManager.contentsOfDirectory(
            at: queueDir, includingPropertiesForKeys: nil,
            options: [.skipsHiddenFiles]))?.filter { $0.hasDirectoryPath } ?? []
        var entries: [ShareQueueEntry] = []
        for dir in dirs.sorted(by: { $0.lastPathComponent < $1.lastPathComponent }) {
            let id = dir.lastPathComponent
            if var meta = readMeta(id: id) {
                if meta.status == .copying, now.timeIntervalSince(meta.enqueuedAt) > copyGrace {
                    // 扩展已死(宽限窗外仍在 copying)→ 折算失败并落盘
                    meta.status = .failed
                    meta.error = ShareQueueConsumer.errorCopyInterrupted
                    try? writeMeta(meta, id: id)
                }
                entries.append(ShareQueueEntry(id: id, meta: meta))
            } else {
                // 孤儿目录(meta 未写)→ 同折算(meta 落盘失败行持久)
                var meta = ShareQueueMeta(
                    targetDeviceId: "", sourceName: "", size: 0,
                    enqueuedAt: now, status: .failed, files: [],
                    error: ShareQueueConsumer.errorCopyInterrupted)
                if let attrs = try? fileManager.attributesOfItem(atPath: dir.path),
                   let created = (attrs[.creationDate] as? Date) {
                    meta.enqueuedAt = created
                }
                try? writeMeta(meta, id: id)
                entries.append(ShareQueueEntry(id: id, meta: meta))
            }
        }
        return entries.sorted { $0.meta.enqueuedAt < $1.meta.enqueuedAt }
    }

    /// 标失败(原因落盘;stagedPaths 在场则保留——重试不重拷)。
    public func markFailed(id: String, error: String, now: Date) throws {
        guard var meta = readMeta(id: id) else {
            throw ShareQueueError.metaMissing(id: id)
        }
        meta.status = .failed
        meta.error = error
        try writeMeta(meta, id: id)
    }

    /// 全 meta 写回(消费面状态迁移用;调用方已持完整 meta)。
    public func writeMetaPublic(_ meta: ShareQueueMeta, id: String) throws {
        try writeMeta(meta, id: id)
    }

    /// 标暂存完成(文件已 move 进收件箱,记录绝对路径;崩溃断点由
    /// staged 状态续消)。
    public func markStaged(id: String, stagedPaths: [String], now: Date) throws {
        guard var meta = readMeta(id: id) else {
            throw ShareQueueError.metaMissing(id: id)
        }
        meta.status = .staged
        meta.stagedPaths = stagedPaths
        try writeMeta(meta, id: id)
    }

    /// 清队列项(发送受理成功后)。
    public func removeItem(id: String, fileManager: FileManager = .default) throws {
        if fileManager.fileExists(atPath: itemDir(id: id).path) {
            try fileManager.removeItem(at: itemDir(id: id))
        }
    }

    // MARK: 内部

    var fileManager: FileManager { .default }

    /// meta 编解码(epoch 秒;扩展↔主 App 两端同策略,键面蛇形)。
    static let metaEncoder: JSONEncoder = {
        let e = JSONEncoder()
        e.outputFormatting = [.sortedKeys]
        e.dateEncodingStrategy = .secondsSince1970
        return e
    }()
    static let metaDecoder: JSONDecoder = {
        let d = JSONDecoder()
        d.dateDecodingStrategy = .secondsSince1970
        return d
    }()

    /// meta 原子写(临时名 + rename;扩展/主 App 双端并发写不可能——
    /// 状态机单向,原子性防的是读侧撞上半截 JSON)。
    func writeMeta(_ meta: ShareQueueMeta, id: String) throws {
        try fileManager.createDirectory(at: itemDir(id: id), withIntermediateDirectories: true)
        let data = try Self.metaEncoder.encode(meta)
        let tmp = metaURL(id: id).appendingPathExtension("tmp")
        try data.write(to: tmp, options: .atomic)
        // atomic write 已保证落盘完整;rename 到位即提交(POSIX 同目录)
        if fileManager.fileExists(atPath: metaURL(id: id).path) {
            try fileManager.removeItem(at: metaURL(id: id))
        }
        try fileManager.moveItem(at: tmp, to: metaURL(id: id))
    }
}

/// 文件 move 进收件箱(消费面;撞名 ` (n)` 顺延,InboxPublisher 同语言)。
/// rename 优先(同卷零拷贝),跨容器 rename 失败回退 copy+remove。
extension ShareQueueStore {
    public static func moveIntoInbox(
        fileURL: URL, name: String, inbox: String, fileManager: FileManager = .default,
    ) throws -> String {
        try fileManager.createDirectory(atPath: inbox, withIntermediateDirectories: true)
        let destPath = uniqueInboxPath(inbox: inbox, name: name, fileManager: fileManager)
        do {
            try fileManager.moveItem(at: fileURL, to: URL(fileURLWithPath: destPath))
        } catch {
            // 跨容器 rename(EPERM/EXDEV 等)回退两步拷贝
            try fileManager.copyItem(at: fileURL, to: URL(fileURLWithPath: destPath))
            try? fileManager.removeItem(at: fileURL)
        }
        return destPath
    }

    /// 撞名顺延:`name.ext` → `name (1).ext`、`name (2).ext` …
    static func uniqueInboxPath(
        inbox: String, name: String, fileManager: FileManager,
    ) -> String {
        let base = inbox + "/" + name
        if !fileManager.fileExists(atPath: base) { return base }
        let ns = name as NSString
        let stem = ns.deletingPathExtension
        let ext = ns.pathExtension
        var n = 1
        while true {
            let candidate = ext.isEmpty
                ? "\(stem) (\(n))"
                : "\(stem) (\(n)).\(ext)"
            let path = inbox + "/" + candidate
            if !fileManager.fileExists(atPath: path) { return path }
            n += 1
        }
    }
}

public enum ShareQueueError: Error, Equatable {
    case metaMissing(id: String)
    case sizeMismatch(name: String, declared: Int64, actual: Int64)
}

// MARK: - 消费面(主 App 回前台;send 闭包注入宿主可测)

public struct ShareQueueConsumer: Sendable {

    /// 失败原因常量(折算态文案;引擎错误原文直存)。
    public static let errorCopyInterrupted = "文件拷贝中断"
    public static let errorSizeMismatch = "文件校验不符"

    public let queue: ShareQueueStore
    /// 收件箱绝对路径(暂存目标;生产 = EngineLayout.inbox)。
    public let inboxPath: String
    /// 发送注入面:(收件箱内绝对路径组, 目标设备 id) → 作业 id。
    /// App 侧接 SendService.send 的 Ruling I 口径;测试注入桩。
    public var send: @Sendable ([String], String) async throws -> String
    public var now: @Sendable () -> Date
    public var copyGrace: TimeInterval

    public init(
        queue: ShareQueueStore, inboxPath: String,
        send: @escaping @Sendable ([String], String) async throws -> String,
        now: @escaping @Sendable () -> Date = { Date() },
        copyGrace: TimeInterval = ShareQueueStore.defaultCopyGrace,
    ) {
        self.queue = queue
        self.inboxPath = inboxPath
        self.send = send
        self.now = now
        self.copyGrace = copyGrace
    }

    public struct Outcome: Equatable, Sendable {
        /// 受理进发送的项数(发送启动成功即算;完成与否归引擎/传输列表)。
        public var consumed: Int
        public init(consumed: Int) { self.consumed = consumed }
    }

    /// 一轮消费(scan → 分档处理;幂等,反复调用安全):
    /// - failed → 跳过(呈现面上浮,重试面另接 ``retryOnce(id:)``——失败
    ///   项不自动重发,重试是用户动作(spec「可重试」口径));
    /// - copying(宽限窗外)→ 已被 scan 折算失败,跳过;
    /// - ready → 逐文件校验 → 暂存 → 发送 → 清队列;
    /// - staged → 校验暂存件在场 → 发送 → 清队列(断点续消)。
    @discardableResult
    public func consumeOnce() async -> Outcome {
        await consumeMany { $0.meta.status == .ready || $0.meta.status == .staged }
    }

    /// 用户重试单条失败项(传输列表「分享转交失败」行的重试入口;重选
    /// 目标后调用,targetDeviceId 可覆盖更新):
    /// - 发送失败型(failed + stagedPaths)→ 校验暂存件在场 → 重发;
    /// - 校验/拷贝中断型(failed 无 stagedPaths)→ 队列内文件逐个复验
    ///   (完整文件过,残件不过)→ 可暂存则重发,否则保持 failed。
    /// true = 发送已受理(成功即清队列项)。
    @discardableResult
    public func retryOnce(id: String, overrideTargetDeviceId: String? = nil) async -> Bool {
        let entries = (try? queue.scan(now: now(), copyGrace: copyGrace)) ?? []
        guard var entry = entries.first(where: { $0.id == id }), entry.meta.status == .failed else {
            return false
        }
        if let overrideTargetDeviceId, !overrideTargetDeviceId.isEmpty {
            entry.meta.targetDeviceId = overrideTargetDeviceId
        }
        return await consume(entry, allowFailed: true)
    }

    func consumeMany(_ eligible: (ShareQueueEntry) -> Bool) async -> Outcome {
        let entries = (try? queue.scan(now: now(), copyGrace: copyGrace)) ?? []
        var consumed = 0
        for entry in entries where eligible(entry) {
            if await consume(entry) {
                consumed += 1
            }
        }
        return Outcome(consumed: consumed)
    }

    /// 单项消费;true = 发送已受理。
    func consume(_ entry: ShareQueueEntry, allowFailed: Bool = false) async -> Bool {
        let fm = FileManager.default
        // ① 取发送路径:ready = 暂存并记录;staged = 断点续用;
        //    failed(重试) = 暂存件在场续用,否则队列内复验再暂存
        let paths: [String]
        switch entry.meta.status {
        case .ready:
            do {
                paths = try stage(entry)
            } catch {
                markFailedQuietly(entry, Self.stageErrorText(of: error))
                return false
            }
        case .staged:
            let recorded = entry.meta.stagedPaths ?? []
            let missing = recorded.contains { !fm.fileExists(atPath: $0) }
            guard !recorded.isEmpty, !missing else {
                markFailedQuietly(entry, ShareQueueConsumer.errorSizeMismatch)
                return false
            }
            paths = recorded
        case .failed where allowFailed:
            let recorded = entry.meta.stagedPaths ?? []
            if !recorded.isEmpty, !recorded.contains(where: { !fm.fileExists(atPath: $0) }) {
                paths = recorded
            } else if recorded.isEmpty {
                // 校验/拷贝中断型:队列内文件复验(完整件过,残件拒)
                do {
                    paths = try stage(entry)
                } catch {
                    markFailedQuietly(entry, Self.stageErrorText(of: error))
                    return false
                }
            } else {
                return false
            }
        default:
            return false
        }
        // ② 发送(引擎收口;App 侧闭包承担 Ruling I/登录态检查)
        do {
            _ = try await send(paths, entry.meta.targetDeviceId)
        } catch {
            // 不静默丢:文件留暂存 + failed 态(重试走 staged 不重拷)
            markFailedQuietly(entry, Self.errorText(of: error))
            return false
        }
        // ③ 受理成功即清(meta 已无用;文件归收件箱/引擎作业管)
        try? queue.removeItem(id: entry.id)
        return true
    }

    /// ready 项暂存:逐文件校验实际尺寸(不符/缺件整单拒收)→ move 进
    /// 收件箱(撞名顺延)→ 落 staged 断点。任一失败抛(调用方折失败态)。
    func stage(_ entry: ShareQueueEntry) throws -> [String] {
        let fm = FileManager.default
        var staged: [String] = []
        for file in entry.meta.files {
            let src = queue.filesDir(id: entry.id).appendingPathComponent(file.name)
            guard fm.fileExists(atPath: src.path) else {
                throw ShareQueueError.sizeMismatch(name: file.name, declared: file.size, actual: -1)
            }
            let attrs = try fm.attributesOfItem(atPath: src.path)
            let actual = (attrs[.size] as? NSNumber)?.int64Value ?? -1
            guard actual == file.size else {
                throw ShareQueueError.sizeMismatch(
                    name: file.name, declared: file.size, actual: actual)
            }
            staged.append(try ShareQueueStore.moveIntoInbox(
                fileURL: src, name: file.name, inbox: inboxPath))
        }
        try queue.markStaged(id: entry.id, stagedPaths: staged, now: now())
        return staged
    }

    func markFailedQuietly(_ entry: ShareQueueEntry, _ message: String) {
        // 合并写回:盘上 meta 为基(stagedPaths 等暂存态在 stage 后才落盘,
        // 不可被扫描时点的内存副本回滚),仅内存侧保留重试目标覆盖
        var meta = queue.readMeta(id: entry.id) ?? entry.meta
        meta.targetDeviceId = entry.meta.targetDeviceId
        meta.status = .failed
        meta.error = message
        try? queue.writeMetaPublic(meta, id: entry.id)
    }

    /// 引擎/App 错误 → 存档文案(RpcError.message 透传,超时兜底;公开供
    /// 扩展直发的 failed 文案复用——R10)。
    public static func errorText(of error: Error) -> String {
        if let rpc = error as? RpcError { return rpc.message }
        if error is RpcGuard.TimeoutError { return "请求超时" }
        return "发送失败:\(error.localizedDescription)"
    }

    /// 暂存错误 → 存档文案(校验不符钉字面;文件系统错误如实携带)。
    static func stageErrorText(of error: Error) -> String {
        if let q = error as? ShareQueueError, case .sizeMismatch = q {
            return ShareQueueConsumer.errorSizeMismatch
        }
        return "暂存失败:\(error.localizedDescription)"
    }
}

// MARK: - App Group 常量(两端共用;spec §5.4)

public enum AppGroup {
    /// App Group 标识(spec §5.4 逐字)。
    public static let identifier = "group.top.mtdss.shared"
}

// MARK: - 字节文案(扩展项目清单/失败行共面)

public enum ShareFormat {
    /// 字节文案(TransfersFormat.speedText 同口径:1024 进制一位小数,
    /// <1024 整数;0 → 「0 B」)。
    public static func bytes(_ count: Int64) -> String {
        if count <= 0 { return "0 B" }
        if count < 1024 { return "\(count) B" }
        let units = ["KB", "MB", "GB", "TB"]
        var v = Double(count) / 1024
        var i = 0
        while v >= 1024 && i < units.count - 1 {
            v /= 1024
            i += 1
        }
        return String(format: "%.1f %@", v, units[i])
    }
}
