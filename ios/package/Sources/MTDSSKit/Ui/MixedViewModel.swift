/// MixedViewModel — 快捷目录混合视图模型(V3-3 Task 15;Windows favfetch.ts/
/// mixedview.ts 引擎的 iOS 镜像,T16 混合视图页消费;返工波 4 §16.4 扩
/// SWR 缓存、§14.2 对齐扩本机源组 local_copy 下载):
/// - **渐进扇出**(favfetch.ts sync 硬复位同象):``reload`` 先 devices 快照
///   (DevicesViewModel 同款限时),快照判离线的成员初始即 offline 桶**零
///   list 请求**;其余 pending 并行 ``RpcGuard/withTimeout`` list(callTimeout
///   与 BrowseViewModel 同值),应答逐成员 upsert(行经 ``visibleRows()``
///   渐进出现,不等齐);
/// - **SWR 缓存**(§16.4,Windows favCache 同象):模块级按 fav 缓存各成员
///   最近一次 ok 应答——``reload`` 先以缓存即时种子(命中成员直接 ok 落
///   行,零 pending,整表瞬时呈现;成员设备现已离线 → 照常 offline 翻桶,
///   离线判定恒以 devices 快照为准),在线成员(含缓存 ok)**全部后台再
///   校验**,应答**差异落位**(条目未变 = 不落不发布,零 @Published 扰动;
///   变了 = 差异 upsert;再校验失败 = 静默保旧,不闪失败面)。失效两路:
///   ①members 签名变化(管理屏编辑/他端同步)→ 该 fav 缓存即弃回快照
///   种子;②登出/被移出 → ``clearFavCache()`` 清场(调用方在登录态事件
///   处调用)。缓存仅内存不落盘,跨 MixedViewModel 实例存活。
/// - **世代守卫**(TS gen 同象):reload 即 +1,迟到应答(gen 不符或成员现
///   状态非 pending 且非「ok→ok 再校验」)整包丢弃;
/// - **truncated 聚合**:任一成员 list truncated 即真(硬重扇出复位;补
///   Windows 侧 deferred 项的 iOS 侧正确性);
/// - **复合键选择**(mixedview MixedRow.key 同构):key = device\0path,
///   同路径跨源两行两 key 并存不合并;
/// - **排序**:文件夹恒前 + 名称/日期/大小/种类,比较器抽自
///   ``BrowseViewModel/less``(同源单份);主比并列以成员添加序收口(TS
///   sortEntries 稳定同象);
/// - **逐组动作**(mixedview.groupRowsByDevice 同象):选中行按来源设备分组
///   (组序 = 渲染序首次出现序,组内保持渲染序),逐组发起,任一组失败即
///   false;下载远端组逐组 transfer.start download(dir = 收件箱哨兵,R12 ①
///   口径),**本机源组转 local_copy**(§14.2 对齐:不再静默跳过——部分
///   本机选中时旧跳过 = 静默丢组;dir = 本机收件箱绝对路径,调用方传
///   recvDir;引擎 local_copy 预检拒绝哨兵),发送跳过 from==to 组(远端
///   拒绝自环),删除全本机组恒可删、含远端组须门开(确认弹窗归 T16,VM
///   只执行:不退选不重载,收口归调用方)。
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
    /// 搜索框:按名称过滤行(空 = 不过滤;大小写不敏感子串,名称域——
    /// ``BrowseViewModel/searchText`` 同源口径)。跨节过滤:零命中节 = 节头
    /// 单行(渲染侧);全选/动作分组天然只及可见行(选择面与浏览页同构)。
    @Published public var searchText = ""
    /// 选择模式开关。
    @Published public private(set) var isSelecting = false
    /// 选中行集(键 = 复合键 device\0path)。
    @Published public private(set) var selection: Set<String> = []

    /// 当前快捷(updateFavorite 仅名变更时只换引用,不重扇出)。
    public private(set) var favorite: Favorite

    /// 本机 device_id(动作分组跳本机组/删除远端门判远端;nil = 未知 →
    /// 下载不跳、删除按远端从门——保守口径)。
    public let selfDeviceId: String?

    private let repo: MtdssRepository
    /// 世代计数器(reload 硬复位即 +1;旧世代应答整包丢弃)。
    private var gen = 0
    /// truncated 成员键集(聚合 ``truncated``;世代重置即清)。
    private var truncatedMembers: Set<String> = []
    /// 当前 fav 的成员签名(reload 时点定格;缓存失效钩子①与再校验落缓存
    /// 同源,Windows curSig 同象)。
    private var curSig: String = ""

    public init(repo: MtdssRepository, favorite: Favorite, selfDeviceId: String?) {
        self.repo = repo
        self.favorite = favorite
        self.selfDeviceId = selfDeviceId
    }

    // MARK: SWR 缓存(§16.4,模块级:跨实例存活;Windows favCache 同象)

    /// 缓存条目:favId → 各成员最近一次 ok 应答(states 仅含 ok 成员)+
    /// 成员签名。签名 = memberKey 串接(␁ 分隔;memberKey 自身 NUL 分隔
    /// 不冲突)——签名不符 = members 变更(管理屏编辑/他端同步),缓存即
    /// 失效。类型全 Sendable 值,访问恒在主 actor(VM 与调用方同面)。
    private struct FavCacheEntry {
        var sig: String
        var states: [MemberState]
    }

    @MainActor private static var favCache: [String: FavCacheEntry] = [:]

    /// 成员签名(缓存失效判定键;␁ 分隔与 memberKey 的 NUL 无歧义)。
    static func membersSig(_ members: [FavMember]) -> String {
        members.map { $0.device + "\u{0000}" + $0.path }.joined(separator: "\u{0001}")
    }

    /// 登出/被移出清场(模块缓存跨会话存活,必须显式清;App 在登录态事件
    /// 处调用——换账号后旧账号的目录内容绝不可瞬时复用)。
    @MainActor public static func clearFavCache() {
        favCache.removeAll()
    }

    /// 缓存落位(ok 应答写入模块缓存;仅 ok 态,失败/离线不写 = 保旧)。
    private func cachePut(member: FavMember, entries: [DirEntry]) {
        let okState = MemberState(member: member, status: .ok, entries: entries)
        if let cur = Self.favCache[favorite.id], cur.sig == curSig {
            Self.favCache[favorite.id] = FavCacheEntry(
                sig: curSig, states: MixedViewModel.upsert(cur.states, member, okState))
        } else {
            Self.favCache[favorite.id] = FavCacheEntry(sig: curSig, states: [okState])
        }
    }

    /// 条目差异判定(§16.4 再校验面):逐条逐字段比——全同 = 未变(不落
    /// 不发布,零 @Published 扰动)。五字段全量(children 为 stat 便利面,
    /// 非 list 应答内容,不参与——Windows FsEntry 五字段同构)。public =
    /// 宿主可测纯面(Kit 测试 import 纪律:不用 @testable)。
    public static func sameEntries(_ a: [DirEntry], _ b: [DirEntry]) -> Bool {
        a.count == b.count && zip(a, b).allSatisfy { x, y in
            x.path == y.path && x.name == y.name && x.size == y.size
                && x.mtimeMs == y.mtimeMs && x.isDir == y.isDir
        }
    }

    /// states 原位 upsert(保序;纯函数)。
    private static func upsert(
        _ states: [MemberState], _ member: FavMember, _ next: MemberState,
    ) -> [MemberState] {
        guard let idx = states.firstIndex(where: { $0.member == member }) else {
            return states + [next]
        }
        var out = states
        out[idx] = next
        return out
    }

    // MARK: 拉取(渐进扇出)

    /// 重扇出(favfetch.ts sync 硬复位同象):devices 快照先定离线桶(零
    /// 请求),其余 pending 并行 list,应答逐成员 upsert;世代 +1,旧世代
    /// 迟到应答整包丢弃。快照拉取失败按「目录不可得」处理 = 全员 offline
    /// (TS 快照里查无此设备 = offline 同象;重试 reload 收口)。快照拉取窗
    /// 即置 loading(iOS 无 devices prop,种子窗比 TS 多一段;commit 后恒由
    /// states 派生)。**SWR 种子**(§16.4):签名命中 = 缓存 ok 成员即时落
    /// 位(零 pending,整表瞬时);签名不符(members 变更)→ 该 fav 缓存
    /// 即弃,退回快照种子。在线成员**全部**(重)拉取 = pending 首拉 + 缓存
    /// ok 后台再校验(差异落位);离线成员从不发 list。
    public func reload() async {
        gen += 1
        let myGen = gen
        loading = true
        let directory = try? await RpcGuard.withTimeout(seconds: Self.devicesTimeout) { [repo] in
            try await repo.devices()
        }
        guard myGen == gen else { return } // 快照在途已换代:本次弃(新 reload 管理状态)
        // 在线表(§13.7 防御口径):快照若含重复 device_id 行(病态上游;
        // 现网服务端 devices 表 device_id PRIMARY KEY 不可产出),按「后行
        // 覆盖前行」折叠(TS `new Map` 同象)——绝不用
        // Dictionary(uniqueKeysWithValues:)(重复键即 trap,用户报告崩溃面)
        var online: [String: Bool] = [:]
        for device in directory?.devices ?? [] {
            online[device.deviceId] = device.online
        }
        curSig = Self.membersSig(favorite.members)
        // 快照分桶 + 缓存种子(mixedview.offlineMembers 同象):离线即定零
        // 请求;缓存命中成员直接 ok(整表瞬时);其余 pending。序 = 成员添加序
        var seeded: [MemberState] = []
        var refetch: [FavMember] = [] // 在线成员全集(首拉 + 再校验同流)
        let cached = Self.favCache[favorite.id]
        let cacheUsable = cached != nil && cached?.sig == curSig
        if !cacheUsable {
            Self.favCache[favorite.id] = nil // 签名不符:缓存即弃(失效钩子①)
        }
        for member in favorite.members {
            guard online[member.device] ?? false else {
                seeded.append(MemberState(member: member, status: .offline, entries: []))
                continue
            }
            refetch.append(member)
            if cacheUsable,
               let hit = cached?.states.first(where: {
                   $0.status == .ok && $0.member == member
               }) {
                seeded.append(hit)
            } else {
                seeded.append(MemberState(member: member, status: .pending, entries: []))
            }
        }
        truncatedMembers = []
        truncated = false
        commit(seeded)
        // 并行扇出(favfetch.ts fetchMember 同象):逐成员独立,一人失败不波及他人;
        // 逐应答差异落位(§16.4:未变零发布,渐进出现语义同前)。
        let favId = favorite.id
        await withTaskGroup(of: (FavMember, MemberStatus, [DirEntry], Bool).self) { group in
            for member in refetch {
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
                if status == .ok {
                    let k = member.device + "\u{0000}" + member.path
                    if trunc { truncatedMembers.insert(k) } else { truncatedMembers.remove(k) }
                    truncated = !truncatedMembers.isEmpty
                }
                apply(favId: favId, member: member, status: status, entries: entries)
            }
        }
    }

    /// 快捷更新(T17/T18 编辑后):members 数组与旧不同 → 硬重扇出(世代随
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
    /// 分组渲染归 view 层(T16),VM 只供 ``states`` + 本投影。搜索过滤在此
    /// 单口(名称域大小写不敏感子串)——全选/动作分组与渲染行同面。
    public func visibleRows() -> [MixedRow] {
        var rows: [MixedRow] = []
        let search = searchText
        for st in states where st.status == .ok {
            for entry in st.entries {
                if !search.isEmpty, !entry.name.localizedCaseInsensitiveContains(search) {
                    continue
                }
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

    // MARK: 逐组动作(mixedview.groupRowsByDevice 同象;行左滑单行 = 单行
    //        集,与选择底栏同一动作函数面,§15.6)

    /// 行集按来源设备分组(组序 = 渲染序首次出现序,组内保持渲染序;入参
    /// 行集来自 ``visibleRows()`` 投影——选择态 = 选中行,左滑档 = 单行)。
    private func groupsOf(_ rows: [MixedRow]) -> [(device: String, paths: [String])] {
        var order: [String] = []
        var byDevice: [String: [String]] = [:]
        for row in rows {
            if byDevice[row.device] == nil {
                order.append(row.device)
                byDevice[row.device] = []
            }
            byDevice[row.device]?.append(row.entry.path)
        }
        return order.map { (device: $0, paths: byDevice[$0] ?? []) }
    }

    /// 选中行(活选,渲染序)。
    private func selectedRows() -> [MixedRow] {
        visibleRows().filter { selection.contains($0.key) }
    }

    /// 下载选中项到本机收件箱:按来源设备逐组发起——**远端组** transfer.start
    /// download(dir = 收件箱哨兵,R12 ① 口径);**本机源组** transfer.start
    /// local_copy(§14.2 对齐:不再静默跳过——「本机文件已在本机」的旧跳过
    /// 在部分本机选中时静默丢组;dir = 本机收件箱绝对路径 ``inboxDir``,
    /// 引擎 local_copy 预检拒绝哨兵,调用方传 AppModel.recvDir 现值;nil =
    /// 收件箱路径未知窗,本机组建档跳过同旧口径)。逐组顺序发起,任一组
    /// 失败即 false(不做早期中止)。
    public func downloadSelected(inboxDir: String?) async -> Bool {
        await downloadRows(selectedRows(), inboxDir: inboxDir)
    }

    /// 下载行集(行左滑档同口;单行 = 单行集)。
    public func downloadRows(_ rows: [MixedRow], inboxDir: String?) async -> Bool {
        var anyFail = false
        for g in groupsOf(rows) {
            if g.device == selfDeviceId {
                guard let inboxDir, !inboxDir.isEmpty else { continue }
                do {
                    _ = try await RpcGuard.withTimeout(seconds: Self.transferStartTimeout) { [repo, g, inboxDir] in
                        try await repo.transferStartLocalCopy(paths: g.paths, dir: inboxDir)
                    }
                } catch {
                    anyFail = true
                }
            } else {
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
        }
        return !anyFail
    }

    /// 发送选中项到对端收件箱:按来源设备逐组 SendService.send 同形调用
    /// (upload {files, to, dir=收件箱哨兵});from==to 组跳过(远端拒绝自环)。
    public func sendSelected(to device: DirectoryDevice) async -> Bool {
        await sendRows(selectedRows(), to: device)
    }

    /// 发送行集(行左滑转发档同口;单行 = 单行集)。
    public func sendRows(_ rows: [MixedRow], to device: DirectoryDevice) async -> Bool {
        var anyFail = false
        for g in groupsOf(rows) where g.device != device.deviceId {
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
        await deleteRows(selectedRows(), allowRemoteDelete: allowRemoteDelete)
    }

    /// 删除行集(行左滑档同口;单行 = 单行集,门口径同选择档)。
    public func deleteRows(_ rows: [MixedRow], allowRemoteDelete: Bool) async -> Bool {
        let groups = groupsOf(rows)
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

    /// 单成员应答落态(SWR 分状态落位,Windows applyMember 同象):
    /// pending 应答照旧 upsert(pending→ok/failed,ok 落缓存);已 ok 成员
    /// 的**后台再校验**应答(§16.4)按差异落位——未变不落(订阅不触发,零
    /// @Published 扰动),变了差异 upsert + 落缓存,失败静默保旧(stale 保
    /// 留,不闪失败面);offline/failed 现状态的迟到应答弃(翻桶/守卫口径
    /// 同 TS applyMember)。loading 恒由 states 现状派生。
    private func apply(favId: String, member: FavMember, status: MemberStatus, entries: [DirEntry]) {
        guard let idx = states.firstIndex(where: { $0.member == member }) else { return }
        let cur = states[idx]
        switch cur.status {
        case .pending:
            if status == .ok { cachePut(member: member, entries: entries) }
            states[idx] = MemberState(member: member, status: status, entries: entries)
        case .ok:
            guard status == .ok else { return } // 再校验失败:静默保旧
            guard !Self.sameEntries(cur.entries, entries) else { return } // 未变:零扰动
            cachePut(member: member, entries: entries)
            states[idx] = MemberState(member: member, status: .ok, entries: entries)
        case .offline, .failed:
            return // 迟到应答弃(翻桶后不该复活)
        }
        loading = states.contains { $0.status == .pending }
    }

    /// 种子落态(commit;loading 派生,TS commit 同象)。
    private func commit(_ newStates: [MemberState]) {
        states = newStates
        loading = newStates.contains { $0.status == .pending }
    }
}
