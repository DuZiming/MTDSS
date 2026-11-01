/// 扩展侧队列写入面(T11,spec §4.7):NSItemProvider → 临时副本(打开
/// 即启,「已在途即可」)→ 点设备后经 Kit ``ShareQueueStore`` 两步拷入
/// App Group(临时名+完成 rename)并落 meta(目标设备+声明尺寸)。
///
/// 约束:扩展秒级寿命 + 内存限额 → 不链引擎、不做网络;仅文件装载与
/// 队列写入。装载 = loadFileRepresentation(系统给的临时文件只在回调内
/// 有效,回调内即拷出)。

import Foundation
import MTDSSKit
import UniformTypeIdentifiers
import UIKit

/// 分享项(装载面产出:UI 清单 + 入队源)。
struct SharePayload: Identifiable {
    let id: UUID
    let name: String
    let size: Int64
    let tmpURL: URL
}

/// 装载/入队失败(可呈现面)。
enum ShareEnqueueError: LocalizedError {
    case noFileRepresentation
    case queueUnavailable
    case copyFailed(underlying: Error)

    var errorDescription: String? {
        switch self {
        case .noFileRepresentation: return "该项不是可发送的文件"
        case .queueUnavailable: return "共享容器不可用(未配置 App Group)"
        case .copyFailed(let underlying): return "入队失败:\(underlying.localizedDescription)"
        }
    }
}

final class ShareQueueWriter {

    /// App Group 队列面(不可得 = 扩展禁用,面板出提示)。
    let store: ShareQueueStore?
    /// 临时副本目录(进程临时区;入队完成/扩展退出即弃)。
    private let stagingDir: URL
    private let fileManager: FileManager

    init(fileManager: FileManager = .default) {
        self.fileManager = fileManager
        store = ShareQueueStore.appGroupRoot(fileManager: fileManager).map { ShareQueueStore(root: $0) }
        stagingDir = fileManager.temporaryDirectory
            .appendingPathComponent("mtdss-share-\(UUID().uuidString)", isDirectory: true)
    }

    // MARK: 装载(打开即启)

    /// attachments → 临时副本清单(逐项尽力装载;失败项计数供 UI 注记,
    /// 不拖垮整单)。
    func loadItems(from context: NSExtensionContext) async -> (items: [SharePayload], failures: Int) {
        let attachments = context.inputItems
            .compactMap { $0 as? NSExtensionItem }
            .flatMap { $0.attachments ?? [] }
        var items: [SharePayload] = []
        var failures = 0
        for provider in attachments {
            do {
                items.append(try await loadPayload(provider: provider))
            } catch {
                failures += 1
            }
        }
        return (items, failures)
    }

    /// 单条 attachment 装载:按注册类型逐个尝试文件表示(图片/通用文件
    /// 都可出文件表示;纯文本/URL 无 → noFileRepresentation)。
    private func loadPayload(provider: NSItemProvider) async throws -> SharePayload {
        let identifiers = provider.registeredTypeIdentifiers
        // 纯文本/URL 类靠后(它们通常无文件表示;具体媒体/文件类优先)
        let ordered = identifiers.sorted { lhs, _ in
            !(UTType(lhs)?.conforms(to: .text) ?? false)
        }
        for identifier in ordered {
            if let payload = try? await loadFileRepresentation(provider: provider, typeIdentifier: identifier) {
                return payload
            }
        }
        throw ShareEnqueueError.noFileRepresentation
    }

    private func loadFileRepresentation(
        provider: NSItemProvider, typeIdentifier: String,
    ) async throws -> SharePayload {
        let type = UTType(typeIdentifier)
        try fileManager.createDirectory(at: stagingDir, withIntermediateDirectories: true)
        let id = UUID()
        let dir = stagingDir
        return try await withCheckedThrowingContinuation { continuation in
            do {
                // 系统临时文件只在回调内有效 → 回调内即拷出
                _ = try provider.loadFileRepresentation(
                    forTypeIdentifier: typeIdentifier,
                ) { url, error in
                    guard let url else {
                        continuation.resume(throwing: error ?? ShareEnqueueError.noFileRepresentation)
                        return
                    }
                    do {
                        let ext = (type?.preferredFilenameExtension ?? url.pathExtension).lowercased()
                        let base = Self.displayName(provider: provider, type: type)
                        let fileName = ext.isEmpty || base.pathExtension.lowercased() == ext
                            ? base
                            : "\(base).\(ext)"
                        let dest = dir.appendingPathComponent("\(id.uuidString)-\(fileName)")
                        try FileManager.default.copyItem(at: url, to: dest)
                        let attrs = try FileManager.default.attributesOfItem(atPath: dest.path)
                        let size = (attrs[.size] as? NSNumber)?.int64Value ?? 0
                        continuation.resume(returning: SharePayload(
                            id: id, name: fileName, size: size, tmpURL: dest))
                    } catch {
                        continuation.resume(throwing: error)
                    }
                }
            } catch {
                // 类型未注册文件表示(同步抛)→ continuation 即刻收口
                continuation.resume(throwing: error)
            }
        }
    }

    /// 展示名(suggestedName 优先;缺省「分享文件」;净化路径分量)。
    static func displayName(provider: NSItemProvider, type: UTType?) -> String {
        let suggested = provider.suggestedName
        let base = suggested?.isEmpty == false
            ? suggested!
            : (type?.preferredFilenameExtension.map { "分享文件.\($0)" } ?? "分享文件")
        return ShareQueueStore.sanitizedName(base)
    }

    // MARK: 入队(点设备;两步拷贝)

    /// 全部装载项两步拷入 App Group 队列并翻 ready。返回入队项数。
    @discardableResult
    func enqueue(items: [SharePayload], targetDeviceId: String) throws -> Int {
        guard let store else { throw ShareEnqueueError.queueUnavailable }
        guard !items.isEmpty else { return 0 }
        let id = UUID().uuidString
        let files = items.map {
            ShareQueueFile(name: ShareQueueStore.sanitizedName($0.name), size: $0.size)
        }
        try store.beginItem(id: id, targetDeviceId: targetDeviceId, files: files, now: Date())
        do {
            for payload in items {
                try store.copyFile(
                    item: id,
                    file: ShareQueueFile(name: payload.name, size: payload.size),
                    from: payload.tmpURL)
            }
            try store.finishItem(id: id, now: Date())
        } catch {
            // 半途失败(容器满等):不落坏项,原样上抛(UI 呈现;队列无残项)
            try? store.removeItem(id: id)
            throw ShareEnqueueError.copyFailed(underlying: error)
        }
        cleanup(items: items)
        return items.count
    }

    /// 临时副本清理(入队成功路径;扩展退出路径由 deinit 兜底)。
    func cleanup(items: [SharePayload]) {
        for payload in items {
            try? fileManager.removeItem(at: payload.tmpURL)
        }
        try? fileManager.removeItem(at: stagingDir)
    }

    deinit {
        try? FileManager.default.removeItem(at: stagingDir)
    }
}
