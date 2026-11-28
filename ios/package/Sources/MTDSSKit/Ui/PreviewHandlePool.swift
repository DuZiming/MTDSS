/// 预览回环句柄池(V4-1 T24):浏览行/磁贴缩略图的 handle 生命周期收口。
/// 安卓 ui/browse/PreviewHandlePool.kt 同构(引用计数/竞态闭合/降级面全同)。
///
/// 生命周期语义(防泄漏是底线;daemon 侧 handles 60s 空闲回收为终极兜底):
/// - **在场 open**:条目入视口(行/磁贴组合)经 ``urlFor`` 取缩略图 URL——
///   池内已有 = 引用计数 +1 复用同 handle;没有 = preview_open 一枚(IPC
///   无批量 op,逐条目开,批量面如实记录)。
/// - **离场 close**:条目出视口(滚动出屏/换页/切形态/退屏)经 ``release``
///   减计数,归零即 preview_close;``releaseAll`` = 屏面卸载全量收口。
/// - **离场竞态闭合**:open 在途遇条目离场 = 调用方任务被取消,``urlFor``
///   在入账前查 `Task.isCancelled`——取消态不入账,半开槽不留(安卓
///   ensureActive 同款;迟到应答即弃,孤儿 handle 由 daemon 60s 兜底)。
/// - **降级面**:open 失败(源不可达/文件已变更 Stale/未授权)→ nil,调用
///   方落类型图标(缩略图缺位只是降级,daemon 同语义);close 失败静默
///   (注入闭包抛错被吞,daemon 60s 空闲回收兜底)。
///
/// 并发纪律:actor 串行化槽表(安卓主 dispatcher 单线程同象)——引用计数
/// 增减与开槽/摘槽互不交错。

import Foundation

public actor PreviewHandlePool {

    private struct Slot {
        let result: PreviewOpenResult
        var refs: Int
    }

    private var slots: [String: Slot] = [:]
    private let open: @Sendable (DirEntry) async throws -> PreviewOpenResult
    private let close: @Sendable (String) async throws -> Void

    /// - Parameters:
    ///   - open: 条目 → preview_open(注入缝:BrowseView 按浏览档装
    ///     device/size/mtime;测试面注入假件)。
    ///   - close: handle → preview_close(池内吞错,失败静默)。
    public init(
        open: @escaping @Sendable (DirEntry) async throws -> PreviewOpenResult,
        close: @escaping @Sendable (String) async throws -> Void,
    ) {
        self.open = open
        self.close = close
    }

    /// 活槽数(单测锚)。
    public var liveCount: Int { slots.count }

    /// 取条目缩略图 URL(引用计数 +1;无 handle 可用 = nil → 图标占位)。
    /// open 在途遇取消(条目离场)= 不入账直返,调用方任务同亡。
    public func urlFor(_ entry: DirEntry) async -> String? {
        if let slot = slots[entry.path] {
            slots[entry.path] = Slot(result: slot.result, refs: slot.refs + 1)
            return BrowseThumbs.thumbUrl(port: slot.result.port, handle: slot.result.handle, token: slot.result.token)
        }
        guard let result = try? await open(entry) else { return nil }
        // 离场竞态:open 已回但调用方任务已取消(条目出视口)→ 不入账,
        // 泄漏闭合(孤儿 handle 归 daemon 60s 空闲回收)。
        guard !Task.isCancelled else { return nil }
        slots[entry.path] = Slot(result: result, refs: 1)
        return BrowseThumbs.thumbUrl(port: result.port, handle: result.handle, token: result.token)
    }

    /// 引用计数 −1(条目离场);归零 = close 并摘槽。未知路径幂等静默。
    public func release(_ entry: DirEntry) async {
        guard let slot = slots[entry.path] else { return }
        if slot.refs > 1 {
            slots[entry.path] = Slot(result: slot.result, refs: slot.refs - 1)
            return
        }
        slots.removeValue(forKey: entry.path)
        try? await close(slot.result.handle) // close 失败静默:daemon 60s 空闲回收兜底
    }

    /// 全量收口(屏面卸载;逐槽摘除并 close,close 同静默面)。
    public func releaseAll() async {
        for path in slots.keys.sorted() {
            // 影子条目:槽只存 path,release 以 path 键,size/mtime 不参与收口面。
            await release(DirEntry(name: path, path: path, size: 0, mtimeMs: 0, isDir: false))
        }
    }
}
