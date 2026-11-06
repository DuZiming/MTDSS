/// FavoritesStore — 常用目录 store + schema + 色板(V3-3 Task 14,TS
/// favorites.ts/favstore.ts 的 Swift 语义镜像):账号级 KV 整单 LWW 的 Kit
/// 收敛面,供 T15 起 UI 消费。
///
/// 同步模型(spec 2026-11-05-mtdss-v3p3-favorites §3):编辑 op 统一走私函
/// ``FavoritesStore.mutate``「kv.get 取最新 → parse 前向守卫 → 纯改 →
/// kv.put 整单回写」,put 成功才落 @Published;put 失败状态保旧值、返回
/// false/0、不自动重试。updated_at 由 mutate 写回前统一盖 now(纯函数不动
/// 时间字段,create 的 now 仅作新条目 created_at)。前向守卫:解析出 v>1 →
/// readOnly,拒绝编辑且绝不回写(防旧版 schema 覆盖新版数据)。data_changed
/// 订阅/登录态接线在 App 侧(T15),store 本体不持会话概念。
///
/// 容差(TS parseFavorites 逐条对齐):无键(not_found)/空串 → 空清单可编辑;
/// 解码失败/非对象/无 v 可读 → readOnly=true + 空;v>1 → readOnly=true +
/// 已知字段尽力解析;updated_at→0、favorites→[]、created_at→0、members→[]
/// 缺失补默认;id/name 非字符串的条目整条丢弃。

import Combine
import Foundation

// MARK: - schema(TS favorites.ts 镜像;wire snake_case)

/// 常用成员:设备 id + 目录路径原串(路径分隔符随目标平台,勿在本层切分)。
public struct FavMember: Codable, Equatable, Hashable, Sendable {
    public let device: String
    public let path: String

    public init(device: String, path: String) {
        self.device = device
        self.path = path
    }
}

/// 单个常用条目:id=客户端生成 16 hex;空成员(0 个)合法;Hashable 供
/// FavoritesRoute 携带(T15+)。
public struct Favorite: Codable, Equatable, Hashable, Sendable, Identifiable {
    public let id: String
    public let name: String
    public let createdAt: Int64
    public let members: [FavMember]

    enum CodingKeys: String, CodingKey {
        case id, name
        case createdAt = "created_at"
        case members
    }

    public init(id: String, name: String, createdAt: Int64, members: [FavMember]) {
        self.id = id
        self.name = name
        self.createdAt = createdAt
        self.members = members
    }
}

/// 整单文档(写回 v 恒为 1;更高 v 见 ``FavoritesParser`` 只读守卫)。
public struct FavoritesDoc: Codable, Equatable, Sendable {
    public let v: Int
    public let updatedAt: Int64
    public let favorites: [Favorite]

    enum CodingKeys: String, CodingKey {
        case v
        case updatedAt = "updated_at"
        case favorites
    }

    public init(v: Int, updatedAt: Int64, favorites: [Favorite]) {
        self.v = v
        self.updatedAt = updatedAt
        self.favorites = favorites
    }
}

// MARK: - 解析(TS parseFavorites 镜像;容差逐条对齐)

/// kv 存值解析:raw 为 nil/空串 → nil(无键);v=1 → 可编辑;v>1 → 只读 +
/// 已知字段尽力解析;解码失败/非对象/无 v 可读 → 只读 + 空文档;字段缺失
/// 按容差补默认。解析恒成功(除非无键返回 nil),失败态全部折进 readOnly。
enum FavoritesParser {
    struct Parsed {
        let doc: FavoritesDoc
        let readOnly: Bool
    }

    /// 空文档:kv 无值时的合法初始态(404=空清单,spec §3.2)。
    static let empty = FavoritesDoc(v: 1, updatedAt: 0, favorites: [])

    static func parse(_ raw: String?) -> Parsed? {
        guard let raw, !raw.isEmpty else { return nil }
        guard let value = try? JSONParser.parse(raw), case .object(let rec) = value else {
            return Parsed(doc: empty, readOnly: true)
        }
        // v 必须是可读整数(缺失/非数值 → 只读 + 空,TS typeof v !== "number" 同象;
        // 整数口径:schema v 恒整,TS 对小数 v 的 2.5>1 边界不在契约面)
        guard case .number(let vRaw)? = rec["v"], let v = Int64(vRaw) else {
            return Parsed(doc: empty, readOnly: true)
        }
        var favorites: [Favorite] = []
        if case .array(let items)? = rec["favorites"] {
            favorites = items.compactMap(parseFavorite)
        }
        var updatedAt: Int64 = 0
        if case .number(let raw2)? = rec["updated_at"], let n = Int64(raw2) { updatedAt = n }
        // 文档 v 规范化为 1(TS 同象):v>1 仅用于 readOnly 裁定,绝不回写
        return Parsed(doc: FavoritesDoc(v: 1, updatedAt: updatedAt, favorites: favorites), readOnly: v > 1)
    }

    /// 尽力解析单个条目:id/name 非字符串(身份与展示缺失)即丢弃;
    /// created_at 缺 → 0;members 缺/含残项 → 逐项过滤(双字符串字段)。
    private static func parseFavorite(_ value: JSONValue) -> Favorite? {
        guard case .object(let rec) = value,
              case .string(let id)? = rec["id"],
              case .string(let name)? = rec["name"] else { return nil }
        var members: [FavMember] = []
        if case .array(let items)? = rec["members"] {
            for item in items {
                if case .object(let m) = item,
                   case .string(let device)? = m["device"],
                   case .string(let path)? = m["path"] {
                    members.append(FavMember(device: device, path: path))
                }
            }
        }
        var createdAt: Int64 = 0
        if case .number(let raw)? = rec["created_at"], let n = Int64(raw) { createdAt = n }
        return Favorite(id: id, name: name, createdAt: createdAt, members: members)
    }
}

// MARK: - 色板(TS DEVICE_COLOR_HEX/deviceColorMap 镜像)

/// 终端标志色板(spec §13.4 返工钦定版:用户逐格挑选 64 色 verbatim,8 色
/// 系×8 明暗档行主序矩阵;§4 的状态色回避限制已由用户裁定作废,含蓝/绿/
/// 红/橙)。无 # 前缀,渲染侧拼 Color;大小写与 TS DEVICE_COLOR_HEX 一致
/// (小写),hex 逐值与 TS 对齐。
public enum FavoritesPalette {
    public static let hexes = [
        "158cd5", "d52c15", "2fd515", "d58515", "1548d5", "d5158f", "15cfd5", "7f15d5",
        "1679b6", "b62916", "2cb616", "b67316", "1641b6", "b6167b", "16b0b6", "6e16b6",
        "1e9deb", "eb361e", "39eb1e", "eb961e", "1e55eb", "eb1ea0", "1ee5eb", "8f1eeb",
        "1a6593", "93281a", "2a931a", "93611a", "1a3a93", "931a67", "1a8f93", "5d1a93",
        "3faef3", "f3543f", "57f33f", "f3a83f", "3f6ff3", "f33fb1", "3fedf3", "a23ff3",
        "1f516f", "6f291f", "2a6f1f", "6f4e1f", "1f356f", "6f1f52", "1f6d6f", "4b1f6f",
        "1f3e51", "51251f", "26511f", "513c1f", "1f2d51", "511f3f", "1f4f51", "3b1f51",
        "1b2c37", "371e1b", "1f371b", "372b1b", "1b2237", "371b2c", "1b3637", "2a1b37",
    ]

    /// 设备 → 标志色:按终端**加入账号顺序**(created_at 升序)取位次
    /// i → hexes[i mod 64];缺 createdAt(旧 daemon)排尾,彼此按 deviceId,
    /// createdAt 并列再按 deviceId。排序与 TS deviceColorMap 逐语义镜像——
    /// TS 侧 device_id 平序用 UTF-16 code-unit `<`,deviceId 是 ASCII 域,
    /// 与 Swift String 序无差异。零服务端状态,三端同算法同结果;≤64 台必
    /// 不撞色,>64 台回绕(spec §13.4)。返回新字典,不改入参数组顺序。
    public static func colorMap(devices: [DirectoryDevice]) -> [String: String] {
        let ordered = devices.sorted { a, b in
            switch (a.createdAt, b.createdAt) {
            case let (l?, r?):
                return l != r ? l < r : a.deviceId < b.deviceId
            case (.some, .none):
                return true
            case (.none, .some):
                return false
            case (.none, .none):
                return a.deviceId < b.deviceId
            }
        }
        var map: [String: String] = [:]
        for (i, d) in ordered.enumerated() {
            map[d.deviceId] = hexes[i % hexes.count]
        }
        return map
    }
}

// MARK: - store(TS createFavoritesCore 镜像)

/// 常用目录 store:编辑 op 统一 mutate「kv.get 取最新 → 前向守卫 → 纯改 →
/// kv.put 整单回写」,put 成功才落 @Published;失败保旧值、返 false/0、不
/// 自动重试(用户重试动作)。busy 门:在途编辑拒绝重入(双击/并发双发
/// lost-update 防线,UI 禁按钮之外的第一道防线)。
@MainActor
public final class FavoritesStore: ObservableObject {

    /// account_kv 键名(spec §3.1,ASCII 定死;= TS FAVORITES_KV_KEY)。
    public static let kvKey = "favorites"

    /// 常用条目清单(创建序;refresh 整单替换,编辑 put 成功才落)。
    @Published public private(set) var favorites: [Favorite] = []
    /// 前向守卫旗(v>1 或解析失败;true 时 UI 禁编辑入口,store 绝不回写)。
    @Published public private(set) var readOnly = false
    /// 一次编辑 mutate 在途(store 级重入门)。
    @Published public private(set) var busy = false

    public init() {}

    // MARK: 拉取

    /// 重拉(服务端为准整单替换):无键(not_found)= 合法空单可编辑;解析
    /// 失败 → readOnly=true + 空;v>1 → readOnly=true + 已知字段尽力解析;
    /// 其他失败(离线/会话失效)保旧值,等下次信号。
    public func refresh(repo: MtdssRepository) async {
        let raw: String?
        do {
            raw = try await repo.kvGet(key: Self.kvKey)
        } catch let e as RpcError where e.kind == "not_found" {
            favorites = []
            readOnly = false
            return
        } catch {
            return // 拉取失败:保旧值,不重试
        }
        guard let parsed = FavoritesParser.parse(raw) else {
            favorites = []
            readOnly = false
            return
        }
        favorites = parsed.doc.favorites
        readOnly = parsed.readOnly
    }

    // MARK: 编辑 op(T4 面;全部 put 成功才落)

    /// 新建常用(命名对话框):尾部追加(创建序),空成员合法;新条目 id
    /// 客户端生成 16 hex,created_at=now。
    public func create(repo: MtdssRepository, name: String) async -> Bool {
        await mutate(repo: repo) { Self.creating($0, name: name, now: Self.nowMillis()) } != nil
    }

    /// 重命名:只改目标条目名;id 不存在 → 等值文档(仍回写,TS 同象)。
    public func rename(repo: MtdssRepository, id: String, to name: String) async -> Bool {
        await mutate(repo: repo) { Self.renaming($0, id: id, to: name) } != nil
    }

    /// 删除常用=解除链接(spec §3.1,绝不触碰远端文件):只删目标条目。
    public func delete(repo: MtdssRepository, id: String) async -> Bool {
        await mutate(repo: repo) { Self.removing($0, id: id) } != nil
    }

    /// 添加成员:目标常用内按 memberKey 去重(含入参数组内部重复),新成员
    /// 尾部追加(添加序),返回实际新增数;零新增(全重复/目标缺失)→ 不
    /// 回写;put 失败返回 0 保旧值。
    public func addMembers(repo: MtdssRepository, id: String, members: [FavMember]) async -> Int {
        var added = 0
        let next = await mutate(repo: repo) { doc in
            let r = Self.addingMembers(doc, id: id, members: members)
            added = r.added
            return r.added == 0 ? nil : r.doc // 零新增 → 不回写
        }
        return next == nil ? 0 : added
    }

    /// 移除成员(解除链接):只删目标常用内匹配 memberKey 的那一项。
    public func removeMember(repo: MtdssRepository, id: String, member: FavMember) async -> Bool {
        await mutate(repo: repo) { Self.removingMember($0, id: id, member: member) } != nil
    }

    // MARK: 编辑统一私函

    /// 编辑统一私函(TS mutate 镜像):前置 kv.get 必取最新(他端可能刚改)
    /// → parse → readOnly 拒(落 readOnly 旗,绝不 kvPut)→ transform 改
    /// (nil = 无可写变更,不回写)→ 盖 updated_at=now → kvPut 整单 → put
    /// 成功才落 @Published;失败状态保旧值、返回 nil、不自动重试。
    private func mutate(
        repo: MtdssRepository,
        transform: (FavoritesDoc) -> FavoritesDoc?,
    ) async -> FavoritesDoc? {
        guard !busy else { return nil }
        busy = true
        defer { busy = false }
        let raw: String?
        do {
            raw = try await repo.kvGet(key: Self.kvKey)
        } catch let e as RpcError where e.kind == "not_found" {
            raw = nil // 无键 = 空单起点(TS parsed null → EMPTY)
        } catch {
            return nil // 编辑前置 kv.get 失败 = 本次编辑动作失败
        }
        let parsed = FavoritesParser.parse(raw)
        if let parsed, parsed.readOnly {
            readOnly = true // 前向守卫:v>1 只读
            return nil
        }
        guard let next = transform(parsed?.doc ?? FavoritesParser.empty) else { return nil }
        let stamped = FavoritesDoc(v: 1, updatedAt: Self.nowMillis(), favorites: next.favorites)
        guard let payload = Self.encode(stamped), (try? await repo.kvPut(key: Self.kvKey, value: payload)) == true else {
            return nil // put 失败:保旧值,不重试
        }
        favorites = stamped.favorites // put 成功才落状态
        readOnly = false
        return stamped
    }

    // MARK: 纯改函数(TS favorites.ts 逐条镜像;一律返回新文档不改入参)

    /// 新建常用:尾部追加(创建序)。
    private static func creating(_ doc: FavoritesDoc, name: String, now: Int64) -> FavoritesDoc {
        let item = Favorite(id: newId(), name: name, createdAt: now, members: [])
        return FavoritesDoc(v: 1, updatedAt: doc.updatedAt, favorites: doc.favorites + [item])
    }

    /// 重命名:只改目标条目名;id 不存在 → 等值返回。
    private static func renaming(_ doc: FavoritesDoc, id: String, to name: String) -> FavoritesDoc {
        FavoritesDoc(v: 1, updatedAt: doc.updatedAt, favorites: doc.favorites.map {
            $0.id == id ? Favorite(id: $0.id, name: name, createdAt: $0.createdAt, members: $0.members) : $0
        })
    }

    /// 删除常用=解除链接:只删目标条目。
    private static func removing(_ doc: FavoritesDoc, id: String) -> FavoritesDoc {
        FavoritesDoc(v: 1, updatedAt: doc.updatedAt, favorites: doc.favorites.filter { $0.id != id })
    }

    /// 成员去重键:NUL 分隔避免 device/path 拼接歧义(TS memberKey 同象;
    /// 同 (device,path) 在该常用内去重,跨常用重复允许)。
    private static func memberKey(_ m: FavMember) -> String { m.device + "\u{0000}" + m.path }

    /// 添加成员:目标内按 memberKey 去重(含入参内部重复),新成员尾部追加
    /// (添加序);added=实际新增数;目标不存在 → added 0、文档不变。
    private static func addingMembers(
        _ doc: FavoritesDoc, id: String, members: [FavMember],
    ) -> (doc: FavoritesDoc, added: Int) {
        var added = 0
        let favorites = doc.favorites.map { fav -> Favorite in
            guard fav.id == id else { return fav }
            var seen = Set(fav.members.map(memberKey))
            var fresh: [FavMember] = []
            for m in members {
                let k = memberKey(m)
                if seen.contains(k) { continue }
                seen.insert(k)
                fresh.append(m)
            }
            added += fresh.count
            return Favorite(id: fav.id, name: fav.name, createdAt: fav.createdAt, members: fav.members + fresh)
        }
        return (FavoritesDoc(v: 1, updatedAt: doc.updatedAt, favorites: favorites), added)
    }

    /// 移除成员(解除链接):只删目标内匹配 memberKey 的那一项。
    private static func removingMember(_ doc: FavoritesDoc, id: String, member: FavMember) -> FavoritesDoc {
        let k = memberKey(member)
        return FavoritesDoc(v: 1, updatedAt: doc.updatedAt, favorites: doc.favorites.map {
            $0.id == id
                ? Favorite(id: $0.id, name: $0.name, createdAt: $0.createdAt, members: $0.members.filter { memberKey($0) != k })
                : $0
        })
    }

    // MARK: 杂项

    /// 生成常用条目 id:系统安全随机 8 字节 → 16 位小写 hex(TS newFavId 同象;
    /// Foundation 面即够,无 UIKit 依赖)。
    private static func newId() -> String {
        var g = SystemRandomNumberGenerator()
        let bytes = (0..<8).map { _ in UInt8.random(in: .min ... .max, using: &g) }
        return bytes.map { String(format: "%02x", $0) }.joined()
    }

    /// 整单 JSON(wire snake_case;声明序 = TS 插入序 v/updated_at/favorites)。
    private static func encode(_ doc: FavoritesDoc) -> String? {
        guard let data = try? JSONEncoder().encode(doc) else { return nil }
        return String(decoding: data, as: UTF8.self)
    }

    /// 毫秒纪元(TS Date.now() 同象)。
    private static func nowMillis() -> Int64 {
        Int64(Date().timeIntervalSince1970 * 1000)
    }
}
