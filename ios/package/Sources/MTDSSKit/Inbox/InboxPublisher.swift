/// 收件箱发布面(P4a 的 iOS 形态,spec §3「收件 = 私有暂存+完成发布」/
/// §5.2 两步 move+rename):消费 daemon 的 `fs.publish` / `fs.query_dest`
/// 事件,经 fdpass 查询通道应答:
/// - **fs.publish**(0x05):move 暂存终件 → 收件箱本名(暂存区唯一名
///   final_rel 由引擎 unique_target 冲突改名携带;App 侧再撞名——发布窗内
///   用户在收件箱造出同名——按 `name (n).ext` 唯一名收口)→ 应答 ok;
///   源缺席/移动失败 → ok:false + 文案透传(引擎「保存失败」收口,暂存
///   保留待重发,§4 失败保留面)。**杀 App 中间态不可见的锚**:文件只在
///   发布(本帧处置)时刻才出现在收件箱(Files App 可见),全程不经
///   收件箱可见路径的中间名。
/// - **fs.query_dest**(0x04):按**收件箱真实行**应答(在 + size 吻合 →
///   exists;暂存件不是正身——绝不 stat 暂存路径,P4a §6.2 同口径)。
///   应答失败不重试(引擎超时按「不在场」保守重收,F7 铁律)。
///
/// 生命周期:App boot 时随 EventHub 订阅流 `run` 起来(事件流终结即退,
/// 与引擎同寿;引擎监督重启后由调用方重挂)。

import Foundation

/// 文件面抽象(宿主单测注入临时目录;生产 = DefaultInboxFilesystem)。
public protocol InboxPublishingFilesystem: Sendable {
    /// move 暂存终件 → 收件箱(rel 逐级建目录;本名在即 ` (n)` 唯一名),
    /// 返回落位绝对路径。源缺席/移动失败抛错(0x05 ok:false 面)。
    func moveAndRename(stagingPath: String, rel: String, inbox: String) throws -> String

    /// 收件箱真实行:rel 在场且尺寸吻合 → (true, size)。暂存路径不碰。
    func presence(rel: String, size: UInt64, inbox: String) -> (exists: Bool, actualSize: UInt64)

    /// 绝对路径的字节数(缺席 → nil;发布落位的尺寸复核用)。
    func size(atPath path: String) -> UInt64?

    /// 目录行公开区建目录(V3-3 返工波 4 X6 修复轮):rel 段(恰带一个
    /// 尾 '/';daemon 清单面同口径,双尾/裸文件行拒)逐级
    /// createDirectory(幂等合并;同名文件占位 → createDirectory 抛错,
    /// 0x05 ok:false 面)。返回建成目录绝对路径。
    func ensureDirectoryTree(rel: String, inbox: String) throws -> String
}

/// 生产文件面(FileManager;同容器 rename 同卷原子,失败即抛)。
public struct DefaultInboxFilesystem: InboxPublishingFilesystem {

    public init() {}

    public func moveAndRename(stagingPath: String, rel: String, inbox: String) throws -> String {
        let fm = FileManager.default
        let target = InboxPublisher.targetPath(inbox: inbox, rel: rel)
        let dir = (target as NSString).deletingLastPathComponent
        try fm.createDirectory(atPath: dir, withIntermediateDirectories: true)
        // 收口名已占(引擎 finalize 后、发布前的窗口内被占)→ 唯一名顺延。
        var finalPath = target
        if fm.fileExists(atPath: finalPath) {
            finalPath = try InboxPublisher.nextAvailable(target: target)
        }
        try fm.moveItem(atPath: stagingPath, toPath: finalPath)
        return finalPath
    }

    public func ensureDirectoryTree(rel: String, inbox: String) throws -> String {
        // 目录行形态:恰一个尾 '/'(daemon 清单面同口径;'a//' 双尾/非目录
        // 行一律拒);剥尾后段面复核(空段/"."/".." 拒—— ".." 严于 daemon,
        // Documents 路径无法表达逃逸段,拒开 + 0x05 如实报)
        guard rel.hasSuffix("/") else { throw CocoaError(.fileWriteInvalidFileName) }
        let trimmed = String(rel.dropLast())
        guard !trimmed.isEmpty else { throw CocoaError(.fileWriteInvalidFileName) }
        let segments = trimmed.split(separator: "/", omittingEmptySubsequences: false).map(String.init)
        guard segments.allSatisfy({ !$0.isEmpty && $0 != "." && $0 != ".." }) else {
            throw CocoaError(.fileWriteInvalidFileName)
        }
        let dir = InboxPublisher.targetPath(inbox: inbox, rel: trimmed)
        try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        return dir
    }

    public func presence(rel: String, size: UInt64, inbox: String) -> (exists: Bool, actualSize: UInt64) {
        let path = InboxPublisher.targetPath(inbox: inbox, rel: rel)
        guard let actual = sizeImpl(path) else {
            return (false, 0)
        }
        return (actual == size, actual)
    }

    public func size(atPath path: String) -> UInt64? {
        sizeImpl(path)
    }

    private func sizeImpl(_ path: String) -> UInt64? {
        guard let attrs = try? FileManager.default.attributesOfItem(atPath: path),
              let bytes = attrs[.size] as? NSNumber else {
            return nil
        }
        return bytes.uint64Value
    }
}

/// 收件应答面:构造后随事件流 run(单消费)。
public struct InboxPublisher: Sendable {

    /// 布局(inbox 发布根 + fdpass socket 派生)。
    public let layout: EngineLayout
    /// fdpass 查询通道(0x04/0x05 应答)。
    public let fdpass: any FdPassAnswering
    /// 文件面(move/在场)。
    public let files: any InboxPublishingFilesystem

    public init(layout: EngineLayout, fdpass: any FdPassAnswering, files: any InboxPublishingFilesystem) {
        self.layout = layout
        self.fdpass = fdpass
        self.files = files
    }

    /// 事件流消费主环:fs.* 应答,其余事件直通忽略(各屏订阅面互不抢流)。
    public func run(_ events: AsyncStream<IpcEvent>) async {
        for await event in events {
            await handle(event)
        }
    }

    /// 单事件处置(测试直驱面)。
    public func handle(_ event: IpcEvent) async {
        switch event {
        case .destQuery(let requestId, let rel, let size):
            await answerQuery(requestId: requestId, rel: rel, size: size)
        case .publish(let requestId, let rel, let stagingPath, let size):
            await answerPublish(requestId: requestId, rel: rel, stagingPath: stagingPath, size: size)
        case .ensureDir(let requestId, let rel):
            await answerEnsureDir(requestId: requestId, rel: rel)
        default:
            break
        }
    }

    /// fs.publish → move + 0x05(失败面文案透传,引擎「保存失败」拼装)。
    func answerPublish(requestId: UInt64, rel: String, stagingPath: String, size: UInt64) async {
        do {
            let landed = try files.moveAndRename(stagingPath: stagingPath, rel: rel, inbox: layout.inbox)
            // 落位尺寸复核(协议前提的终端复核;不符按失败收口——暂存件已
            // 移走的事实由重发分区自愈,记录 done 未 published)。
            if files.size(atPath: landed) == size {
                await sendPublishResult(requestId: requestId, ok: true, error: nil)
            } else {
                await sendPublishResult(
                    requestId: requestId, ok: false,
                    error: "发布落位尺寸不符: \(landed)",
                )
            }
        } catch {
            await sendPublishResult(requestId: requestId, ok: false, error: "\(error)")
        }
    }

    /// fs.ensure_dir → 公开区 create_dir_all + 0x05(V3-3 返工波 4 X6
    /// 修复轮;应答与发布同帧形状)。失败文案透传(引擎「保存失败」收口)
    /// ——目录行不经暂存/发布管线:目录无暂存件无记录,旧路径落私有暂存
    /// 区永不发布、永不清理,无限累积(评审 Important)。
    func answerEnsureDir(requestId: UInt64, rel: String) async {
        do {
            _ = try files.ensureDirectoryTree(rel: rel, inbox: layout.inbox)
            await sendPublishResult(requestId: requestId, ok: true, error: nil)
        } catch {
            await sendPublishResult(requestId: requestId, ok: false, error: "\(error)")
        }
    }

    /// fs.query_dest → 0x04(收件箱真实行;应答失败仅留痕——引擎 3s 窗
    /// 超时按「不在场」保守重收,失败方向安全)。
    func answerQuery(requestId: UInt64, rel: String, size: UInt64) async {
        let (exists, actual) = files.presence(rel: rel, size: size, inbox: layout.inbox)
        do {
            _ = try await fdpass.send(
                kind: FdPassKind.destQuery,
                json: [
                    "exists": exists,
                    "request_id": requestId,
                    "size": exists ? Int(clamping: actual) : 0,
                ],
            )
        } catch {
            // F7:宁可漏应答(引擎重收),不可假应答。
        }
    }

    func sendPublishResult(requestId: UInt64, ok: Bool, error: String?) async {
        var payload: [String: Any] = ["ok": ok, "request_id": requestId]
        if let error, !error.isEmpty {
            payload["error"] = error
        }
        do {
            let ack = try await fdpass.send(kind: FdPassKind.publishResult, json: payload)
            if ack.kind == FdPassKind.ack, ack.ok == false {
                // 等待方已超时/已交付:引擎侧按「保存失败」收口,重发可再分区。
            }
        } catch {
            // 应答未达 = 引擎超时收口(暂存保留);无重试面(F7 方向安全)。
        }
    }

    // MARK: 路径组装(发布路径面,单测钉)

    /// 收件箱目标路径:rel 的 '/' 分量逐级落位(发布名与 0x04 键同源)。
    public static func targetPath(inbox: String, rel: String) -> String {
        let parts = rel.split(separator: "/").map(String.init)
        guard !parts.isEmpty else { return inbox }
        return ([inbox] + parts).joined(separator: "/")
    }

    /// 撞名顺延:`name.ext` → `name (1).ext`、`name (2).ext` …(目录分量
    /// 原样保留,只改末段文件名)。
    public static func nextAvailable(target: String) throws -> String {
        let ns = target as NSString
        let dir = ns.deletingLastPathComponent
        let name = ns.lastPathComponent
        let stem = (name as NSString).deletingPathExtension
        let ext = (name as NSString).pathExtension
        for n in 1...9_999 {
            let candidate = ext.isEmpty
                ? "\(stem) (\(n))"
                : "\(stem) (\(n)).\(ext)"
            let path = dir.isEmpty ? candidate : dir + "/" + candidate
            if !FileManager.default.fileExists(atPath: path) {
                return path
            }
        }
        throw CocoaError(.fileWriteFileExists)
    }
}

/// ack payload 的 ok 读面(受理回执检查)。
extension FdPassFrame {
    var ok: Bool {
        (try? JSONSerialization.jsonObject(with: payload) as? [String: Any])
            .flatMap { $0["ok"] as? Bool } ?? false
    }
}
