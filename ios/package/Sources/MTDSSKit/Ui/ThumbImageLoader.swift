/// 缩略图内存缓存/装载器(V4-1 T24 fix round 1 Important #1):NSCache 按
/// URL 键 = **唯一缓存面**(Coil 内存缓存同语义)。
///
/// 为什么不用 URLCache:daemon /thumb 应答仅 Content-Type(无 Cache-Control/
/// Expires/Last-Modified),按 RFC 7234 NSURLCache **不得存储**——带头的
/// 20MB URLCache 实际一枚不落,滚动回场即全量 preview_open + 重拉 + 重解码
/// (图标闪面)。本缓存补上 Coil 等价的内存面:会话内同 URL 复进视口零重拉
/// (daemon 侧 handles 按键复用,同条目在场句柄 URL 稳定 → 命中);NSCache
/// 内存压力自逐(LRU 近似)+ countLimit 帽,零外部依赖、零 daemon 改动。
///
/// 失败面:非 200/解码失败**不落缓存**(每次调用可重试,缩略图缺位只是
/// 降级面);fetch 抛错(含取消)同不出图不落缓存。
///
/// 测试缝:fetch 闭包注入(计数假件钉「零二次网络调用」),cache 注入
/// (NSCache 同型)。

import Foundation
import ImageIO

/// 缩略图装载会话(ephemeral:URLSession 缓存层显式关闭——缓存面只在
/// NSCache,语义单一;15s 请求超时 = 回环口常识余量)。
enum ThumbSession {
    static let value: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.urlCache = nil
        config.requestCachePolicy = .reloadIgnoringLocalCacheData
        config.timeoutIntervalForRequest = 15
        return URLSession(configuration: config)
    }()
}

public final class ThumbImageLoader: @unchecked Sendable {

    /// 默认缓存:256 枚帽(NSCache 无字节帽,枚数近似 Coil 20MB 量级;
    /// 内存压力系统自逐,先于 countLimit)。
    static func defaultCache() -> NSCache<NSString, CGImage> {
        let cache = NSCache<NSString, CGImage>()
        cache.countLimit = 256
        return cache
    }

    private let cache: NSCache<NSString, CGImage>
    private let fetch: @Sendable (URL) async throws -> (Data, URLResponse)

    /// - Parameters:
    ///   - cache: 缓存(nil = 默认 256 枚帽缓存;测试可注入同型空缓存)。
    ///   - fetch: 传输缝(nil = ``ThumbSession`` 回环 GET;测试注入计数假件)。
    ///   默认走 init 体内装配(public init 默认参不得引用 internal 符号)。
    public init(
        cache: NSCache<NSString, CGImage>? = nil,
        fetch: (@Sendable (URL) async throws -> (Data, URLResponse))? = nil,
    ) {
        self.cache = cache ?? Self.defaultCache()
        self.fetch = fetch ?? { url in
            try await ThumbSession.value.data(from: url)
        }
    }

    /// App 全局共享(缓存跨屏复用:同条目句柄 URL 在任何浏览页复进视口皆命中)。
    public static let shared = ThumbImageLoader()

    /// 缓存帽(单测锚)。
    public var cacheCountLimit: Int { cache.countLimit }

    /// 取条目缩略图:命中缓存直返(零网络);未中 = fetch → 200 校验 → 解码
    /// → 落缓存。失败(nil)不落缓存,下次调用可重试。
    public func image(for url: URL) async -> CGImage? {
        let key = url.absoluteString as NSString
        if let hit = cache.object(forKey: key) {
            return hit
        }
        guard let (data, response) = try? await fetch(url),
              let http = response as? HTTPURLResponse, http.statusCode == 200,
              let image = Self.decode(data) else {
            return nil
        }
        cache.setObject(image, forKey: key)
        return image
    }

    /// 字节 → CGImage(ImageIO 解码,PNG/JPEG/HEIC 全平台同缝)。
    public static func decode(_ data: Data) -> CGImage? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
        return CGImageSourceCreateImageAtIndex(source, 0, nil)
    }
}
