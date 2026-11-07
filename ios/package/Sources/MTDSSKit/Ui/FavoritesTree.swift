/// FavoritesTree — 快捷树「成员内容合并」纯投影层(V3-3 返工二 §14.1,
/// 2026-11-06 钦定:展开快捷目录条目 → 各**在线**成员目录**内容的并集**
/// (文件与文件夹混排、目录恒前 + 名称升序)为其树子级,推翻 §13.8 的
/// 成员目录节点形态;Windows ui/src/lib/favtree.ts 镜像,iOS 首页
/// FavoritesRootList 消费)。
///
/// 本模块只做投影,不发请求:拉取纪律(offline 零请求、逐成员 list 渐进
/// 并入、世代守卫)归宿主(首页视图侧派生 states——``MixedViewModel/MemberState``
/// 复用为输入面,混合视图同一状态象)。全部纯函数一律返回新值,不改入参;
/// 宿主可测(纯 MTDSSKit,无 SwiftUI)。
///
/// 排序裁定:侧栏树无排序状态,固定「目录恒前 + 名称升序」(Windows
/// FAV_TREE_SORT 同象;比较器 = ``BrowseViewModel/less`` 单源同份)。主比
/// 并列以成员添加序收口(Swift sort 无稳定性契约,显式下标决胜,TS
/// sortEntries 稳定同象)。同路径跨源两行并存不合并(行键 =
/// device\0entry.path,``MixedViewModel/MixedRow`` 同构,NUL 分隔无歧义);
/// 行另携成员位次——重复成员容差(同 (device,path) 两成员),行身份两两
/// 必不同。离线/失败成员条目缺席(§14.1:离线成员的条目缺席),
/// ``unavailableCount`` = offline + failed(注记行「N 个成员离线」的计数
/// 来源;ok / pending 不计——混合视图 unavailableCount 同口径,failed
/// 成员条目同样缺席,并入同一注记不单开错误行)。

import Foundation

@MainActor
public enum FavoritesTree {

    /// 树固定排序:名称升序(目录恒前由 ``BrowseViewModel/less`` 判定)。
    public static let sort: BrowseSort = .name

    /// 合并投影单行:条目 + 来源设备 + 成员位次 + 复合行键。
    public struct Row: Equatable, Sendable {
        public let entry: DirEntry
        public let device: String
        /// 成员添加序位次(0 起;states 序 = 成员添加序,并列决胜后原位
        /// 保留——首页据此组链身份,重复成员两行身份必不同)。
        public let memberIndex: Int
        /// 复合行键 = device\0entry.path(同 path 跨源两行 key 必不同——
        /// 同名并存,spec §5.2 靠来源色点区分)。
        public let key: String

        public init(entry: DirEntry, device: String, memberIndex: Int) {
            self.entry = entry
            self.device = device
            self.memberIndex = memberIndex
            self.key = device + "\u{0000}" + entry.path
        }
    }

    /// 成员内容合并(§14.1;Windows mergeMemberListings 同象):各 ok 成员
    /// 条目平铺(states 序 = 成员添加序)→ 统一重排(目录恒前 + 名称升序,
    /// 跨源按名交织;主比并列显式下标决胜 = 成员添加序)。任意到达时刻
    /// 调用输出均正确(渐进渲染)。入参由调用方构造:entries 先行隐藏
    /// 过滤(缓存存全量,渲染层滤——Windows 同裁定);offline/failed/
    /// pending 成员条目缺席。
    public static func rootRows(_ states: [MixedViewModel.MemberState]) -> [Row] {
        var pairs: [(row: Row, index: Int)] = []
        for (si, st) in states.enumerated() where st.status == .ok {
            for entry in st.entries {
                pairs.append((Row(entry: entry, device: st.member.device, memberIndex: si), si))
            }
        }
        pairs.sort { a, b in
            if BrowseViewModel.less(a.row.entry, b.row.entry, by: sort) { return true }
            if BrowseViewModel.less(b.row.entry, a.row.entry, by: sort) { return false }
            return a.index < b.index
        }
        return pairs.map(\.row)
    }

    /// 条目缺席成员计数 = offline + failed(注记行「N 个成员离线」;
    /// ok / pending 不计,Windows favtree.absentMemberCount 同口径)。
    public static func unavailableCount(_ states: [MixedViewModel.MemberState]) -> Int {
        states.filter { $0.status == .offline || $0.status == .failed }.count
    }
}
