/// MixedViewModel — 常用目录混合视图模型(V3-3 Task 15;Windows favfetch.ts/
/// mixedview.ts 引擎的 iOS 镜像,T16 混合视图页消费):
/// - **渐进扇出**(favfetch.ts sync 硬复位同象):``reload`` 先 devices 快照
///   (DevicesViewModel 同款限时),快照判离线的成员初始即 offline 桶**零
///   list 请求**;其余 pending 并行 ``RpcGuard/withTimeout`` list(callTimeout
///   与 BrowseViewModel 同值),应答逐成员 upsert(行经 ``visibleRows()``
///   渐进出现,不等齐);
/// - **世代守卫**(TS gen 同象):reload 即 +1,迟到应答(gen 不符或成员现
///   状态非 pending)整包丢弃;
/// - **truncated 聚合**:任一成员 list truncated 即真(硬重扇出复位;补
///   Windows 侧 deferred 项的 iOS 侧正确性);
/// - **复合键选择**(mixedview MixedRow.key 同构):key = device\0path,
///   同路径跨源两行两 key 并存不合并;
/// - **排序**:文件夹恒前 + 名称/日期/大小/种类,比较器抽自
///   ``BrowseViewModel/less``(同源单份);主比并列以成员添加序收口(TS
///   sortEntries 稳定同象);
/// - **逐组动作**(mixedview.groupRowsByDevice 同象):选中行按来源设备分组
///   (组序 = 渲染序首次出现序,组内保持渲染序),逐组发起,任一组失败即
///   false;下载跳过本机组(dir = 收件箱哨兵,R12 ① 口径),发送跳过
///   from==to 组(远端拒绝自环),删除全本机组恒可删、含远端组须门开
///   (确认弹窗归 T16,VM 只执行:不退选不重载,收口归调用方)。
///
/// 每个仓库调用均经 ``RpcGuard/withTimeout`` 限时。宿主可测:纯 MTDSSKit,
/// 无 SwiftUI。

import Combine
import Foundation

@MainActor
public final class MixedViewModel: ObservableObject {

    /// 单成员 list/删除调用限时(秒;= ``BrowseViewModel/callTimeout`` 同值,
    /// T15 brief 口径)。
    static let callTimeout: TimeInterval = BrowseViewModel.callTimeout
    /// transfer.start 受理调用限时(秒;= ``SendService/transferStartTimeout``
    /// 同值——冷对端首跳握手窗,同 SendService 三受理点口径)。
    static let transferStartTimeout: TimeInterval = SendService.transferStartTimeout
    /// devices 快照调用限时(秒;= ``DevicesViewModel/callTimeout`` 同值,
    /// brief ruling:设备拉取用 Kit 既有同款)。
    static let devicesTimeout: TimeInterval = DevicesViewModel.callTimeout

    // MARK: 成员状态面(mixedview.ts 镜像)

    /// 成员拉取状态:pending=在途;ok=已并入;offline=设备快照判定离线;
    /// failed=list 失败/超时。
    public enum MemberStatus: Equatable, Sendable {
        case pending
        case ok
        case offline
        case failed
    }

    /// 单成员状态:entries 仅 ok 态承载列表结果(其余态恒 [])。
    public struct MemberState: Identifiable, Equatable, Sendable {
        public let member: FavMember
        public private(set) var status: MemberStatus
        public private(set) var entries: [DirEntry]

        /// 成员身份(复合键,NUL 分隔与 FavoritesStore memberKey 同构)。
        public var id: String { member.device + "\u{0000}" + member.path }

        public init(member: FavMember, status: MemberStatus, entries: [DirEntry]) {
            self.member = member
            self.status = status
            self.entries = entries
        }
    }

    /// 混合视图单行:条目 + 来源设备 + 行键。
    public struct MixedRow: Identifiable, Equatable, Sendable {
        public let entry: DirEntry
        public let device: String
        /// 行键 = device\0entry.path(同 path 跨源两行 key 必不同——同名并存,
        /// spec §5.2 靠来源色点区分)。
        public let key: String

        public var id: String { key }

        public init(entry: DirEntry, device: String) {
            self.entry = entry
            self.device = device
            self.key = device + "\u{0000}" + entry.path
        }
    }

    // MARK: 状态(UI 订阅)

    /// 各成员拉取状态(序 = 成员添加序;应答原位 upsert 保序)。
    @Published public private(set) var states: [MemberState] = []
    /// 恒由 states 现状派生(任一 pending 即真;TS seed/commit 同象)。
    @Published public private(set) var loading = false
    /// 任一成员 list truncated 聚合(硬重扇出复位)。
    @Published public private(set) var truncated = false
    /// 排序(默认 名称 升序;文件夹恒前)。
    @Published public var sort: BrowseSort = .name
    /// 选择模式开关。
    @Published public private(set) var isSelecting = false
    /// 选中行集(键 = 复合键 device\0path)。
    @Published public private(set) var selection: Set<String> = []

    /// 当前常用(updateFavorite 仅名变更时只换引用,不重扇出)。
    public private(set) var favorite: Favorite

    /// 本机 device_id(动作分组跳本机组/删除远端门判远端;nil = 未知 →
    /// 下载不跳、删除按远端从门——保守口径)。
    public let selfDeviceId: String?

    private let repo: MtdssRepository
    /// 世代计数器(reload 硬复位即 +1;旧世代应答整包丢弃)。
    private var gen = 0
    /// truncated 成员键集(聚合 ``truncated``;世代重置即清)。
    private var truncatedMembers: Set<String> = []

    public init(repo: MtdssRepository, favorite: Favorite, selfDeviceId: String?) {
        self.repo = repo
        self.favorite = favorite
        self.selfDeviceId = selfDeviceId
    }

    // MARK: 拉取(渐进扇出)

    /// 重扇出(favfetch.ts sync 硬复位同象):devices 快照先定离线桶(零
    /// 请求),其余 pending 并行 list,应答逐成员 upsert;世代 +1,旧世代
    /// 迟到应答整包丢弃。快照拉取失败按「目录不可得」处理 = 全员 offline
    /// (TS 快照里查无此设备 = offline 同象;重试 reload 收口)。快照拉取窗
    /// 即置 loading(iOS 无 devices prop,种子窗比 TS 多一段;commit 后恒由
    /// states 派生)。
    public func reload() async {
        gen += 1
        let myGen = gen
        loading = true
        let directory = try? await RpcGuard.withTimeout(seconds: Self.devicesTimeout) { [repo] in
            try await repo.devices()
        }
        guard myGen == gen else { return } // 快照在途已换代:本次弃(新 reload 管理状态)
        let online = Dictionary(uniqueKeysWithValues: (directory?.devices ?? []).map { ($0.deviceId, $0.online) })
        // 快照分桶(mixedview.offlineMembers 同象):离线即定零请求;序 = 成员添加序
        var seeded: [MemberState] = []
        var pending: [FavMember] = []
        for member in favorite.members {
            if online[member.device] ?? false {
                seeded.append(MemberState(member: member, status: .pending, entries: []))
                pending.append(member)
            } else {
                seeded.append(MemberState(member: member, status: .offline, entries: []))
            }
        }
        truncatedMembers = []
        truncated = false
        commit(seeded)
        // 并行扇出(favfetch.ts fetchMember 同象):逐成员独立,一人失败不波及他人;
        // 逐应答 upsert(行渐进出现,不等齐)。
        await withTaskGroup(of: (FavMember, MemberStatus, [DirEntry], Bool).self) { group in
            for member in pending {
                let repo = self.repo
                group.addTask {
                    do {
                        let result = try await RpcGuard.withTimeout(seconds: Self.callTimeout) {
                            try await repo.list(device: member.device, path: member.path)
                        }
                        return (member, .ok, result.entries, result.truncated)
                    } catch {
                        return (member, .failed, [], false)
                    }
                }
            }
            for await (member, status, entries, trunc) in group {
                guard myGen == gen else { continue } // 世代已翻:余下应答一并弃(组收口照常)
                guard state(of: member)?.status == .pending else { continue } // 成员现状态守卫(TS applyMember 同款)
                apply(member, status: status, entries: entries)
                if status == .ok {
                    let k = member.device + "\u{0000}" + member.path
                    if trunc { truncatedMembers.insert(k) } else { truncatedMembers.remove(k) }
                    truncated = !truncatedMembers.isEmpty
                }
            }
        }
    }

    /// 常用更新(T17/T18 编辑后):members 数组与旧不同 → 硬重扇出(世代随
    /// reload +1,旧应答弃);仅名变 → 只换引用不重扇出。扇出经 Task
    /// (``reload`` 为 async,本口同步返回,调用方无需等待)。
    public func updateFavorite(_ fav: Favorite) {
        let membersChanged = fav.members != favorite.members
        favorite = fav
        if membersChanged {
            Task { await self.reload() }
        }
    }

    // MARK: 投影

    /// 混合排序投影(mixedview.mixedRows 同象):各 ok 成员行平铺(成员添加
    /// 序)→ 统一重排(文件夹恒前,比较器 = ``BrowseViewModel/less`` 同源)→
    /// 映射回行。主比并列以原序收口(成员添加序,TS sortEntries 稳定同象;
    /// Swift sort 无稳定性契约,显式下标决胜)。任意到达时刻调用输出均正确;
    /// 分组渲染归 view 层(T16),VM 只供 ``states`` + 本投影。
    public func visibleRows() -> [MixedRow] {
        var rows: [MixedRow] = []
        for st in states where st.status == .ok {
            for entry in st.entries {
                rows.append(MixedRow(entry: entry, device: st.member.device))
            }
        }
        var pairs = rows.enumerated().map { (row: $0.element, index: $0.offset) }
        pairs.sort { a, b in
            if BrowseViewModel.less(a.row.entry, b.row.entry, by: sort) { return true }
            if BrowseViewModel.less(b.row.entry, a.row.entry, by: sort) { return false }
            return a.index < b.index
        }
        return pairs.map(\.row)
    }

    // MARK: 选择模式(复合键;BrowseViewModel 同款语义)

    /// 进入选择模式(选择集清空)。
    public func enterSelection() {
        isSelecting = true
        selection = []
    }

    /// 退出选择模式(取消/完成的收尾;选择集清空)。
    public func exitSelection() {
        isSelecting = false
        selection = []
    }

    /// 行点按切换选中态(键 = 复合键 device\0path)。
    public func toggle(_ key: String) {
        if selection.contains(key) {
            selection.remove(key)
        } else {
            selection.insert(key)
        }
    }

    /// 全选可见行(已全选再点 = 清空,iOS 文件 App 惯用;BrowseViewModel 同款)。
    public func selectAll() {
        let all = visibleRows().map(\.key)
        if all.allSatisfy(selection.contains) && !all.isEmpty {
            selection.removeAll()
        } else {
            selection = Set(all)
        }
    }

    // MARK: 逐组动作(mixedview.groupRowsByDevice 同象)

    /// 选中行按来源设备分组(组序 = 渲染序首次出现序,组内保持渲染序)。
    private func selectedGroups() -> [(device: String, paths: [String])] {
        var order: [String] = []
        var byDevice: [String: [String]] = [:]
        for row in visibleRows() where selection.contains(row.key) {
            if byDevice[row.device] == nil {
                order.append(row.device)
                byDevice[row.device] = []
            }
            byDevice[row.device]?.append(row.entry.path)
        }
        return order.map { (device: $0, paths: byDevice[$0] ?? []) }
    }

    /// 下载选中项到本机收件箱:按来源设备逐组 transfer.start download,
    /// dir = 收件箱哨兵(R12 ① 口径,引擎侧统一语义);本机组跳过(下载
    /// 自己 = 无意义)。逐组顺序发起,任一组失败即 false(不做早期中止)。
    public func downloadSelected() async -> Bool {
        var anyFail = false
        for g in selectedGroups() where g.device != selfDeviceId {
            do {
                _ = try await RpcGuard.withTimeout(seconds: Self.transferStartTimeout) { [repo, g] in
                    try await repo.transferStartDownload(
                        device: g.device, paths: g.paths, dir: SendService.inboxSentinel,
                    )
                }
            } catch {
                anyFail = true
            }
        }
        return !anyFail
    }

    /// 发送选中项到对端收件箱:按来源设备逐组 SendService.send 同形调用
    /// (upload {files, to, dir=收件箱哨兵});from==to 组跳过(远端拒绝自环)。
    public func sendSelected(to device: DirectoryDevice) async -> Bool {
        var anyFail = false
        for g in selectedGroups() where g.device != device.deviceId {
            do {
                _ = try await RpcGuard.withTimeout(seconds: Self.transferStartTimeout) { [repo, g, device] in
                    try await repo.transferStartUpload(
                        files: g.paths, to: device.deviceId, dir: SendService.inboxSentinel,
                    )
                }
            } catch {
                anyFail = true
            }
        }
        return !anyFail
    }

    /// 删除选中项:全本机组恒可删;选中含远端组须门开(门关 = 整批拒绝零
    /// 外发,返回 false);逐组 ctrl delete 同形(force = true,iOS 引擎恒
    /// 永久删);组内部分失败(failed 表非空)同计失败。确认弹窗归 UI(T16),
    /// VM 只执行:不退选不重载,收口归调用方。
    public func deleteSelected(allowRemoteDelete: Bool) async -> Bool {
        let groups = selectedGroups()
        let selfId = selfDeviceId
        if groups.contains(where: { $0.device != selfId }), !allowRemoteDelete {
            return false // 门关 + 含远端:整批拒绝(全本机组恒可删)
        }
        var anyFail = false
        for g in groups {
            do {
                let result = try await RpcGuard.withTimeout(seconds: Self.callTimeout) { [repo, g] in
                    try await repo.delete(device: g.device, paths: g.paths, force: true)
                }
                if !result.failed.isEmpty { anyFail = true }
            } catch {
                anyFail = true
            }
        }
        return !anyFail
    }

    // MARK: 内部

    /// 成员现状态(mixedview memberKey 匹配同象;成员集内 (device,path) 唯一)。
    private func state(of member: FavMember) -> MemberState? {
        states.first { $0.member == member }
    }

    /// 单成员应答落态(原位替换保成员添加序;loading 恒由 states 现状派生)。
    private func apply(_ member: FavMember, status: MemberStatus, entries: [DirEntry]) {
        guard let idx = states.firstIndex(where: { $0.member == member }) else { return }
        states[idx] = MemberState(member: member, status: status, entries: entries)
        loading = states.contains { $0.status == .pending }
    }

    /// 种子落态(commit;loading 派生,TS commit 同象)。
    private func commit(_ newStates: [MemberState]) {
        states = newStates
        loading = newStates.contains { $0.status == .pending }
    }
}
