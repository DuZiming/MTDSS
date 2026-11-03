/// 直发暂存(R10,brief 架构 ③):分享项两步拷入 App Group
/// `{root}/send-staging/<批>/`(`.part` 临时名 → 同目录 rename 完成形态,
/// 尺寸守卫与 ``ShareQueueStore`` 两步拷同锚),作为引擎发送源;扩展 home
/// 的 `engine-roots.conf` 授权该暂存根(桥 start 读入,守卫收口)。
///
/// 批生命周期:面板装载即清孤儿批(上次会话残批 = 死流,不再可达)→
/// 点设备建新批拷入 → 直发成功/取消 = 批清弃;降级队列 = ``ShareQueueStore/
/// adoptFiles(_:targetDeviceId:files:fromDirectory:now:)`` 整批 **rename 收编**
/// (同卷零拷贝)。纯 Foundation,宿主可测。
import Foundation

public struct SendStaging: Sendable {

    /// 暂存根目录名(App Group 容器下)。
    public static let dirName = "send-staging"

    /// 暂存根 = `{App Group 容器}/send-staging`。
    public static func appGroupRoot(
        groupRoot: URL,
    ) -> SendStaging {
        SendStaging(root: groupRoot.appendingPathComponent(dirName, isDirectory: true))
    }

    public let root: URL

    public init(root: URL) {
        self.root = root
    }

    /// 生产便利:App Group 容器不可得 = 直发面整体禁用(调用方降级队列)。
    public static func appGroupRoot(
        identifier: String = AppGroup.identifier, fileManager: FileManager = .default,
    ) -> SendStaging? {
        fileManager.containerURL(forSecurityApplicationGroupIdentifier: identifier)
            .map { SendStaging(root: $0.appendingPathComponent(dirName, isDirectory: true)) }
    }

    // MARK: 批生命周期

    /// 建新批目录(`{root}/<批 id>/`)。
    public func newBatch(id: String) throws -> URL {
        let batch = root.appendingPathComponent(id, isDirectory: true)
        try FileManager.default.createDirectory(at: batch, withIntermediateDirectories: true)
        return batch
    }

    /// 批内文件最终路径(发送源路径组用)。
    public func filePath(in batch: URL, name: String) -> String {
        batch.appendingPathComponent(ShareQueueStore.sanitizedName(name)).path
    }

    /// 批清弃(直发完成/取消;不存在幂等)。
    public func removeBatch(_ batch: URL, fileManager: FileManager = .default) {
        try? fileManager.removeItem(at: batch)
    }

    /// 孤儿批清扫(面板装载时;`keeping` 之外的批全清——上次扩展会话的
    /// 残批不再有活流可达,数据仍在分享源可重分享)。
    public func purgeOrphans(
        keeping: URL?, olderThan: TimeInterval, now: Date, fileManager: FileManager = .default,
    ) {
        let dirs = (try? fileManager.contentsOfDirectory(
            at: root, includingPropertiesForKeys: [.contentModificationDateKey],
            options: [.skipsHiddenFiles]))?.filter { $0.hasDirectoryPath } ?? []
        for dir in dirs where dir.path != keeping?.path {
            let modified = (try? dir.resourceValues(forKeys: [.contentModificationDateKey]))?
                .contentModificationDate
            // 无时间戳(异常形态)按过期清(残批本就死流)
            if modified == nil || now.timeIntervalSince(modified!) > olderThan {
                try? fileManager.removeItem(at: dir)
            }
        }
    }

    // MARK: 两步暂存(尺寸守卫同 ShareQueueStore.copyFile)

    /// 单文件两步拷入批(`.part` → rename;源实际尺寸 ≠ 声明即拒,不入
    /// 坏件)。装载面已把分享项拷到宿主临时区,此处自临时区入批。
    public func stageFile(
        batch: URL, file: ShareQueueFile, from source: URL,
        fileManager: FileManager = .default,
    ) throws {
        let name = ShareQueueStore.sanitizedName(file.name)
        let dest = batch.appendingPathComponent(name)
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
}

// MARK: 降级收编(直发不可行 → 整批入队列)

extension ShareQueueStore {
    /// 直发暂存批整批收编为队列项(**move 非拷**:批与队列同在 App Group
    /// 卷内,rename 零拷贝——扩展秒级寿命下不做二次 I/O)。逐文件尺寸复验
    /// (与 copyFile 守卫同锚),任一不符整单拒(不留坏项,批原样保留)。
    /// 成功 = 队列项 `.ready`,主 App 下次打开照常消费(既有 ShareConsumer)。
    public func adoptFiles(
        item id: String, targetDeviceId: String, files: [ShareQueueFile],
        fromDirectory dir: URL, now: Date, fileManager: FileManager = .default,
    ) throws {
        try beginItem(id: id, targetDeviceId: targetDeviceId, files: files, now: now)
        do {
            for file in files {
                let name = Self.sanitizedName(file.name)
                let src = dir.appendingPathComponent(name)
                let attrs = try fileManager.attributesOfItem(atPath: src.path)
                let actual = (attrs[.size] as? NSNumber)?.int64Value ?? -1
                guard actual == file.size else {
                    throw ShareQueueError.sizeMismatch(name: name, declared: file.size, actual: actual)
                }
                let dest = filesDir(id: id).appendingPathComponent(name)
                if fileManager.fileExists(atPath: dest.path) {
                    try fileManager.removeItem(at: dest)
                }
                try fileManager.moveItem(at: src, to: dest)
            }
            try finishItem(id: id, now: now)
        } catch {
            // 不留半套队列项;暂存批原样保留(调用方可重试或呈现错误)
            try? removeItem(id: id)
            throw error
        }
    }

    /// 未暂存面整单入队(R10 降级「不可行/超帽」路;T11 旧路语义:两步
    /// 拷贝全部文件翻 ready,半途失败移除整项不落坏项)。`files` 与
    /// `sources` 平行(声明面与装载面临时副本一一对应)。
    public func enqueueFiles(
        id: String, targetDeviceId: String, files: [ShareQueueFile],
        sources: [URL], now: Date, fileManager: FileManager = .default,
    ) throws {
        precondition(files.count == sources.count, "声明面与源面必须一一对应")
        try beginItem(id: id, targetDeviceId: targetDeviceId, files: files, now: now)
        do {
            for (file, source) in zip(files, sources) {
                try copyFile(item: id, file: file, from: source)
            }
            try finishItem(id: id, now: now)
        } catch {
            try? removeItem(id: id)
            throw error
        }
    }
}
