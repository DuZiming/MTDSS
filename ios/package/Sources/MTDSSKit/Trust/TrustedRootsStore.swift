/// TrustedRootsStore — 信任目录账本(v2-P5 T10,spec §3「授权目录集」App 侧
/// 正身):
/// - **安全域书签**:UIDocumentPicker/系统目录选择器授权的目录以
///   security-scoped bookmark 持久化(trust-roots.json,App Support——引擎
///   home 模拟器反复清空亦不伤账本);重装/容器迁移后经书签再解析(失效
///   = 解析抛错 → 剔除并回写,fail-closed 坏根缩小授权集;stale=true =
///   仍可用,用解析路径)。
/// - **注入消费面**:解析结果经 ``EngineRootsConf/write(home:paths:)`` 落
///   `{home}/engine-roots.conf`,引擎 start 注入授权集(Ruling L:桥读入与
///   Documents 并集;增删信任目录 = 写账本 + 写 conf + 引擎重启,重启
///   ~1-2s,信任变更低频可接受)。展示/管理面只在设置 → 信任目录
///   (2026-11-01 Ruling N:信任目录不投影首页——首页唯一展示项 = 收件箱)。
/// - security-scoped startAccessing 是 iOS 面(协议化):``BookmarkCodec``
///   生产用 ``FoundationBookmarkCodec``(iOS/macOS 均 plain 书签——iOS 端
///   `.withSecurityScope` 实测 unavailable,安全域语义由选择器 URL 出身
///   携带 + ``SecurityScopeAccess`` 持访问;宿主测试走真 Foundation 往返)。

import Foundation
import Observation

/// 信任目录条目(授权时快照:显示名/解析路径 + 书签本体)。
public struct TrustedRoot: Equatable, Sendable, Identifiable, Codable {
    /// 显示名(授权时最后一段路径;后续目录改名不追)。
    public let name: String
    /// 授权时解析路径快照(展示/去重;引擎侧以 resolveAll 的现解析为准)。
    public let path: String
    /// 安全域书签(security-scoped bookmark data)。
    public let bookmark: Data

    public init(name: String, path: String, bookmark: Data) {
        self.name = name
        self.path = path
        self.bookmark = bookmark
    }

    public var id: String { path }
}

/// 书签解析结果(stale = 系统提示书签需刷新;iOS 容器迁移后常见,路径仍可用)。
public struct ResolvedBookmark: Equatable, Sendable {
    public let url: URL
    public let stale: Bool

    public init(url: URL, stale: Bool) {
        self.url = url
        self.stale = stale
    }
}

/// 书签编解码抽象(startAccessing/权限面在 iOS;宿主测试注入同接口实现)。
public protocol BookmarkCodec: Sendable {
    func bookmarkData(for url: URL) throws -> Data
    func resolve(_ data: Data) throws -> ResolvedBookmark
}

/// security-scoped 起动访问抽象(iOS 面:选择器授权的目录须 App 持
/// startAccessing,进程内引擎的 std::fs 读取才落沙箱授权内;协议化注入,
/// 宿主测试走 Foundation 空实现)。
public protocol SecurityScopeAccess: Sendable {
    func start(url: URL)
    func stop(url: URL)
}

/// Foundation 实现(startAccessing/stopAccessing;非作用域 URL 返回
/// false/无副作用,无害)。
public struct FoundationSecurityScope: SecurityScopeAccess {
    public init() {}

    public func start(url: URL) {
        _ = url.startAccessingSecurityScopedResource()
    }

    public func stop(url: URL) {
        url.stopAccessingSecurityScopedResource()
    }
}

/// Foundation 书签实现(iOS 与 macOS 均为 plain 书签——实测 iOS 26 SDK
/// `.withSecurityScope` 不可用(iOS 端 unavailable):选择器授权 URL 的
/// 安全域语义由 URL 出身携带,书签持久化后以 ``SecurityScopeAccess``
/// 起动访问;encode→resolve 真 Foundation 往返)。
public struct FoundationBookmarkCodec: BookmarkCodec {
    public init() {}

    public func bookmarkData(for url: URL) throws -> Data {
        try url.bookmarkData(
            options: [],
            includingResourceValuesForKeys: nil,
            relativeTo: nil,
        )
    }

    public func resolve(_ data: Data) throws -> ResolvedBookmark {
        var stale = false
        let url = try URL(
            resolvingBookmarkData: data,
            options: [],
            relativeTo: nil,
            bookmarkDataIsStale: &stale,
        )
        return ResolvedBookmark(url: url, stale: stale)
    }
}

/// 信任目录账本(@MainActor;视图/装配共享)。
@MainActor
@Observable
public final class TrustedRootsStore {

    /// 账本条目(插入序;UI 列表同序)。
    public private(set) var roots: [TrustedRoot] = []

    private let ledgerURL: URL
    private let codec: any BookmarkCodec
    /// security-scoped 起动访问(iOS 面;持住即进程内引擎可读)。
    private let scope: any SecurityScopeAccess
    /// 已持访问的解析路径集(startAccessing 与账本条目同生命周期)。
    private var accessedPaths: Set<String> = []

    public init(
        ledgerURL: URL,
        codec: any BookmarkCodec = FoundationBookmarkCodec(),
        scope: any SecurityScopeAccess = FoundationSecurityScope(),
    ) {
        self.ledgerURL = ledgerURL
        self.codec = codec
        self.scope = scope
    }

    /// 装配时载入账本(文件不存在 = 空账本常态;损坏按空收口 fail-closed)。
    public func load() {
        guard let data = try? Data(contentsOf: ledgerURL) else { return }
        roots = (try? JSONDecoder().decode([TrustedRoot].self, from: data)) ?? []
    }

    /// 授权目录(书签编码失败抛错 → UI 呈现);重复授权幂等(同路径不重记)。
    /// 成功即持 security-scoped 起动访问(进程内引擎读取授权的前提)。
    @discardableResult
    public func add(url: URL) throws -> TrustedRoot {
        let data = try codec.bookmarkData(for: url)
        let root = TrustedRoot(
            name: url.lastPathComponent,
            path: url.standardizedFileURL.path,
            bookmark: data,
        )
        if !roots.contains(root) {
            roots.append(root)
        }
        scope.start(url: url)
        accessedPaths.insert(root.path)
        try persist()
        return root
    }

    /// 移除信任目录(账本回写;引擎侧授权随 conf 重写 + 重启生效;
    /// security-scoped 访问随之释放)。
    public func remove(_ root: TrustedRoot) {
        roots.removeAll { $0 == root }
        releaseAccess(of: root.path)
        try? persist()
    }

    /// 解析当前账本 → 有效 (root, 解析路径):
    /// - 解析抛错(目录已删/容器迁移后书签失效)= 剔除并回写账本(下次
    ///   启动不再出现);
    /// - stale=true(系统建议刷新)保留,使用现解析路径;
    /// - 存活根持 security-scoped 起动访问(boot 装配路径:杀 App 重启后
    ///   作用域需重持)。
    public func resolveAll() -> [(root: TrustedRoot, path: String)] {
        var alive: [TrustedRoot] = []
        var out: [(root: TrustedRoot, path: String)] = []
        for root in roots {
            if let resolved = try? codec.resolve(root.bookmark) {
                alive.append(root)
                out.append((root, resolved.url.standardizedFileURL.path))
            }
        }
        if alive.count != roots.count {
            roots = alive
            try? persist()
        }
        for (root, path) in out where !accessedPaths.contains(path) {
            if let resolved = try? codec.resolve(root.bookmark) {
                scope.start(url: resolved.url)
                accessedPaths.insert(path)
            }
        }
        return out
    }

    /// 释放指定路径的起动访问(无其他条目映射同路径时)。
    private func releaseAccess(of path: String) {
        guard !roots.contains(where: { $0.path == path }) else { return }
        if accessedPaths.remove(path) != nil {
            scope.stop(url: URL(fileURLWithPath: path))
        }
    }

    private func persist() throws {
        let data = try JSONEncoder().encode(roots)
        try data.write(to: ledgerURL, options: .atomic)
    }
}
