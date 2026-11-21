/// 浏览页视图形态(V4-1 T24;钦定 mockup mobile-browse.html **列表默认** +
/// mobile-tiles-v2.html 「···」切平铺,UserDefaults 跨次进页记忆):
/// - ``BrowseViewMode/list`` 列表:通栏行(图片/视频 = 40pt 缩略图方图行 +
///   视频时长角标,文件夹/文档沿用现行行图标素净体系);
/// - ``BrowseViewMode/tiles`` 平铺:LazyVGrid 4 列磁贴(图标槽/缩略图/时长
///   角标/名字一行居中截断)。
/// 安卓 ui/browse/BrowseViewMode.kt 同构(键值/缺省/翻转单源漂移防护)。

import Foundation

/// 视图形态(键值 = UserDefaults 档面字面)。
public enum BrowseViewMode: String, CaseIterable, Sendable {
    case list
    case tiles

    /// 切往的另一形态(「···」菜单项词面 = 本属性的另一形态名)。
    public var other: BrowseViewMode {
        self == .list ? .tiles : .list
    }

    /// 缺省形态 = 列表(钦定;档面缺席/损坏同归此)。
    public static let `default`: BrowseViewMode = .list

    /// 档面值 → 形态(只认 `list|tiles` 逐字;其余一律缺省列表,不猜)。
    public static func fromKey(_ raw: String?) -> BrowseViewMode {
        guard let raw else { return .default }
        return BrowseViewMode(rawValue: raw) ?? .default
    }
}

/// 视图形态持久化缝(单测注入内存/suite 实现;实机 = UserDefaults 单键)。
public protocol BrowseViewPersistence: Sendable {
    func load() -> String?
    func save(_ value: String)
}

/// UserDefaults 持久化实现(独立键,与登录/账号档案分立)。
struct UserDefaultsBrowseViewPersistence: BrowseViewPersistence {
    let defaults: UserDefaults
    let key: String

    func load() -> String? {
        defaults.string(forKey: key)
    }

    func save(_ value: String) {
        defaults.set(value, forKey: key)
    }
}

/// 浏览视图形态记忆(V4-1 T24):UserDefaults 单键
/// `browse_view_mode`(值 `list|tiles`),「···」切换即时落档。存储选型沿
/// AccountStore 同裁定:单值同步小读写,UserDefaults 零额外依赖。
///
/// **档值不冻结在构造**(安卓 T22 fix round 1 Important #1 同课):store 为
/// 屏级共享记忆,``load`` 每次现查档——save 后新推入的浏览页(收件箱/
/// 远端浏览等)立即读到最新形态,跨屏共享的声明才成立。
public struct BrowseViewModeStore: Sendable {

    /// 档键(brief 钉死逐字)。
    public static let key = "browse_view_mode"

    private let persistence: any BrowseViewPersistence

    /// 实机入口:standard UserDefaults。
    public init(defaults: UserDefaults) {
        self.persistence = UserDefaultsBrowseViewPersistence(defaults: defaults, key: Self.key)
    }

    /// 测试入口:注入持久化缝。
    public init(persistence: any BrowseViewPersistence) {
        self.persistence = persistence
    }

    /// 当前形态(每次读现查档;档面损坏降级列表默认,不炸面)。
    public func load() -> BrowseViewMode {
        BrowseViewMode.fromKey(persistence.load())
    }

    /// 切换落档(「···」消费;UserDefaults 写不抛,缺位只是降级面)。
    public func save(_ mode: BrowseViewMode) {
        persistence.save(mode.rawValue)
    }
}
