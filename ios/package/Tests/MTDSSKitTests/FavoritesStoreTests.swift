import MTDSSKit
import XCTest

/// FavoritesStore 行为测试(V3-3 Task 14,TS favstore.test.ts 六场景 + T3
/// 色板的 Swift 镜像,直驱 store 无会话概念——登录翻转/data_changed 接线在
/// App 侧 T15):
/// ①refresh not_found→空清单可编辑;②v:2 前向守卫→readOnly、create false、
/// kv.put 零调用;③create 前置 kv.get 取最新(注入他端并发新文档,put=双方
/// 条目+新条目);④kvPut 失败→保旧值、不自动重试;⑤addMembers 入参内去重
/// 计数/尾追顺序/零新增不回写;⑥色板 §13.4 钦定 64 值 + 加入顺序
/// (created_at 升序)位次 + 缺戳排尾 + 第 65 台回绕=第 1 色;
/// ⑦rename/removeMember/delete 只动目标条目(其余逐字不动)+store 统一盖
/// updated_at;另补 busy 重入门(在途编辑拒新编辑)。
@MainActor
final class FavoritesStoreTests: XCTestCase {

    // MARK: kv 伪 daemon 脚本

    /// kv.get/kv.put 应答脚本(镜像 StepScript 手法):内存键值仓 + 逐次调用
    /// 留痕(线程安全)。kv.get 未命中 = typed not_found 错误帧(照 golden
    /// kv.get.resp.notfound.json 形);kv.put 落内存仓并留痕值(零调用断言与
    /// put payload 反解析用);putFailure = true 回错误帧;getDelaySeconds
    /// 非 0 时 kv.get 延迟应答(busy 重入门测试用)。
    private final class KvScript: @unchecked Sendable {
        private let lock = NSLock()
        private var store: [String: String] = [:]
        private var gets: [String] = []
        private var puts: [(key: String, value: String)] = []
        var putFailure = false
        var getDelaySeconds: TimeInterval = 0

        func set(_ key: String, _ value: String) { lock.withLock { store[key] = value } }
        var kvGetCount: Int { lock.withLock { gets.count } }
        var kvPutCount: Int { lock.withLock { puts.count } }
        var putValues: [String] { lock.withLock { puts.map(\.value) } }

        static func handler(_ script: KvScript) -> FakeDaemon.LineHandler {
            { conn, line in
                guard let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self)),
                      let method = try? frame.req("method").asString(),
                      case .number(let idRaw)? = frame["id"] else {
                    return
                }
                let id = JSONValue.number(idRaw)
                let params = frame["params"] ?? .object([:])
                switch method {
                case Ops.kvGet:
                    let key = (try? params.req("key").asString()) ?? ""
                    script.recordGet(key) // 请求到达即留痕(延迟前):在途窗口可观测
                    if script.getDelaySeconds > 0 { Thread.sleep(forTimeInterval: script.getDelaySeconds) }
                    if let value = script.lockedValue(key) {
                        conn.sendLine(Self.result(id: id, ["value": .string(value)]))
                    } else {
                        conn.sendLine(Self.error(id: id, message: "无此键: \(key)"))
                    }
                case Ops.kvPut:
                    let key = (try? params.req("key").asString()) ?? ""
                    let value = (try? params.req("value").asString()) ?? ""
                    script.recordAttempt(key: key, value: value) // 调用留痕(失败也计,恰一次断言用)
                    if script.putFailure {
                        conn.sendLine(Self.error(id: id, message: "kv 落盘失败(测试注入)"))
                        return
                    }
                    script.commitPut(key: key, value: value) // UPSERT 落内存仓(后续 get 读新值)
                    conn.sendLine(Self.result(id: id, ["ok": .bool(true)]))
                default:
                    conn.sendLine(Self.result(id: id, [:]))
                }
            }
        }

        // 锁内读写收口(非 async 上下文直触 NSLock 不宜,同房规收口到同步方法)。
        fileprivate func lockedValue(_ key: String) -> String? { lock.withLock { store[key] } }
        fileprivate func recordGet(_ key: String) { lock.withLock { gets.append(key) } }
        fileprivate func recordAttempt(key: String, value: String) { lock.withLock { puts.append((key, value)) } }
        fileprivate func commitPut(key: String, value: String) { lock.withLock { store[key] = value } }

        static func result(id: JSONValue, _ result: [String: JSONValue]) -> Data {
            Data((JSONValue.object(["id": id, "result": .object(result)]).canonical + "\n").utf8)
        }

        static func error(id: JSONValue, message: String) -> Data {
            Data((JSONValue.object(["id": id, "error": .object([
                "code": .number("1"),
                "message": .string(message),
                "data": .object(["kind": .string("not_found")]),
            ])]).canonical + "\n").utf8)
        }
    }

    // MARK: 夹具

    /// 种子历史戳(seed 文档固定此值,断言 store 有盖新戳用)。
    private let t0: Int64 = 1_700_000_000_000

    private func makeStore(_ fake: FakeDaemon) -> (FavoritesStore, MtdssRepository) {
        (FavoritesStore(), MtdssRepository(client: IpcClient(endpoint: fake)))
    }

    private func fav(_ id: String, _ name: String, members: [FavMember] = []) -> Favorite {
        Favorite(id: id, name: name, createdAt: t0, members: members)
    }

    /// 构造 kv 存值串(wire snake_case;updated_at 固定历史值便于断言盖新戳)。
    private func seedDoc(_ favorites: [Favorite], updatedAt: Int64 = 1_700_000_000_000) -> String {
        let data = try! JSONEncoder().encode(FavoritesDoc(v: 1, updatedAt: updatedAt, favorites: favorites))
        return String(decoding: data, as: UTF8.self)
    }

    private func decodePut(_ raw: String) throws -> FavoritesDoc {
        try JSONDecoder().decode(FavoritesDoc.self, from: Data(raw.utf8))
    }

    // MARK: ①refresh:not_found → 空清单可编辑

    func testRefreshNotFoundYieldsEmptyEditable() async throws {
        let fake = FakeDaemon()
        let script = KvScript()
        fake.onLine = KvScript.handler(script)
        let (store, repo) = makeStore(fake)

        await store.refresh(repo: repo)
        XCTAssertEqual(script.kvGetCount, 1)
        XCTAssertTrue(store.favorites.isEmpty, "无键(404)= 合法空单")
        XCTAssertFalse(store.readOnly, "空单可编辑")
        XCTAssertFalse(store.busy)
    }

    // MARK: ②v:2 前向守卫:readOnly + create 拒 + kv.put 零调用

    func testV2ForwardGuardReadOnlyAndZeroPut() async throws {
        let fake = FakeDaemon()
        let script = KvScript()
        fake.onLine = KvScript.handler(script)
        script.set(
            FavoritesStore.kvKey,
            #"{"v":2,"updated_at":42,"favorites":[{"id":"new-v2","name":"新v2条目","created_at":42,"members":[]}]}"#,
        )
        let (store, repo) = makeStore(fake)

        await store.refresh(repo: repo)
        XCTAssertTrue(store.readOnly, "v>1 只读")
        XCTAssertEqual(store.favorites.map(\.name), ["新v2条目"], "已知字段尽力解析")

        let ok = await store.create(repo: repo, name: "本地新建")
        XCTAssertFalse(ok, "只读拒编辑")
        XCTAssertTrue(store.readOnly, "守卫旗保持")
        XCTAssertEqual(script.kvGetCount, 2, "编辑前置仍取一次(守卫在取后裁定)")
        XCTAssertEqual(script.kvPutCount, 0, "前向守卫绝不回写")
        XCTAssertFalse(store.busy)
    }

    // MARK: ③create 前置取最新:两次不同 kv.get,put=并发条目+新条目

    func testCreatePrefetchesLatestAndPutsMergedDoc() async throws {
        let fake = FakeDaemon()
        let script = KvScript()
        fake.onLine = KvScript.handler(script)
        script.set(FavoritesStore.kvKey, seedDoc([fav("seed-1", "已有条目")]))
        let (store, repo) = makeStore(fake)

        await store.refresh(repo: repo)
        XCTAssertEqual(store.favorites.count, 1)

        // 注入他端并发新文档:编辑前置 kv.get 必读到
        script.set(
            FavoritesStore.kvKey,
            seedDoc([fav("seed-1", "已有条目"), fav("seed-2", "他端并发新增")]),
        )
        let ok = await store.create(repo: repo, name: "本地新建")
        XCTAssertTrue(ok)
        XCTAssertEqual(script.kvGetCount, 2, "编辑前置必再取一次")
        XCTAssertEqual(script.kvPutCount, 1)

        let put = try decodePut(script.putValues[0])
        XCTAssertEqual(put.v, 1)
        XCTAssertEqual(put.favorites.map(\.name), ["已有条目", "他端并发新增", "本地新建"], "他端条目+新条目合并回写")
        XCTAssertGreaterThanOrEqual(put.updatedAt, t0, "store 写回前统一盖新戳")
        // 新条目:id=16 位小写 hex、空成员合法、created_at=now、尾部追加(创建序)
        let created = try XCTUnwrap(put.favorites.last)
        XCTAssertEqual(created.name, "本地新建")
        XCTAssertTrue(
            created.id.range(of: #"^[0-9a-f]{16}$"#, options: .regularExpression) != nil,
            "id 应为 16 位小写 hex:\(created.id)",
        )
        XCTAssertTrue(created.members.isEmpty)
        XCTAssertGreaterThanOrEqual(created.createdAt, t0)
        // put 成功才落状态:本地整单与 put 一致
        XCTAssertEqual(store.favorites.map(\.id), put.favorites.map(\.id))
        XCTAssertFalse(store.readOnly)
        XCTAssertFalse(store.busy)
    }

    // MARK: ④kvPut 失败:保旧值、不自动重试

    func testPutFailureKeepsOldValueNoRetry() async throws {
        let fake = FakeDaemon()
        let script = KvScript()
        fake.onLine = KvScript.handler(script)
        script.set(FavoritesStore.kvKey, seedDoc([fav("keep-1", "旧值")]))
        let (store, repo) = makeStore(fake)
        await store.refresh(repo: repo)

        script.putFailure = true
        let ok = await store.create(repo: repo, name: "不会落盘")
        XCTAssertFalse(ok)
        XCTAssertEqual(script.kvPutCount, 1, "无自动重试")
        XCTAssertEqual(store.favorites.map(\.name), ["旧值"], "put 失败保旧值")
        XCTAssertFalse(store.busy)
    }

    // MARK: ⑤addMembers:入参内去重计数 / 尾追顺序 / 零新增不回写

    func testAddMembersDedupeCountAndAppendOrder() async throws {
        let fake = FakeDaemon()
        let script = KvScript()
        fake.onLine = KvScript.handler(script)
        let m1 = FavMember(device: "dev-1", path: "C:\\go")
        let m2 = FavMember(device: "dev-1", path: "C:\\docs")
        script.set(FavoritesStore.kvKey, seedDoc([fav("f1", "项目A")]))
        let (store, repo) = makeStore(fake)
        await store.refresh(repo: repo)

        // 入参 [m1, m1, m2]:m1 重复取一、m2 新增 → 新增 2,尾追保持添加序
        let n1 = await store.addMembers(repo: repo, id: "f1", members: [m1, m1, m2])
        XCTAssertEqual(n1, 2, "入参内 (device,path) 去重计数")
        XCTAssertEqual(script.kvPutCount, 1)
        let put1 = try decodePut(script.putValues[0])
        XCTAssertEqual(put1.favorites[0].members, [m1, m2], "新成员尾部追加(添加序)")

        // kv.put 已落内存仓:再取即含成员 → 全重复 = 零新增不回写
        let n2 = await store.addMembers(repo: repo, id: "f1", members: [m1])
        XCTAssertEqual(n2, 0)
        XCTAssertEqual(script.kvPutCount, 1, "零新增不回写")

        let n3 = await store.addMembers(repo: repo, id: "missing", members: [m1])
        XCTAssertEqual(n3, 0, "目标缺失计 0")
        XCTAssertEqual(script.kvPutCount, 1)
        XCTAssertEqual(store.favorites[0].members, [m1, m2])
        XCTAssertFalse(store.busy)
    }

    // MARK: ⑥色板:§13.4 钦定 64 值 + 加入顺序位次 + 缺戳排尾 + >64 回绕

    func testPaletteHexesVerbatim() {
        XCTAssertEqual(FavoritesPalette.hexes.count, 64, "钦定 64 色(spec §13.4)")
        XCTAssertEqual(FavoritesPalette.hexes.first, "158cd5", "首值 verbatim")
        XCTAssertEqual(FavoritesPalette.hexes.last, "2a1b37", "末值 verbatim")
        XCTAssertEqual(FavoritesPalette.hexes[7], "7f15d5", "第 1 色系末档(采样)")
        XCTAssertEqual(FavoritesPalette.hexes[8], "1679b6", "第 2 色系首档(行主序采样)")
        XCTAssertEqual(FavoritesPalette.hexes[32], "3faef3", "第 5 色系首档(采样)")
        XCTAssertEqual(Set(FavoritesPalette.hexes).count, 64, "64 值互不重复")
    }

    func testColorMapJoinOrderMissingLastAndWraparound() {
        func dev(_ id: String, _ createdAt: Int64?) -> DirectoryDevice {
            DirectoryDevice(deviceId: id, name: nil, platform: nil, online: true, createdAt: createdAt)
        }
        // 乱序输入:created_at 升序取位;并列按 device_id;缺戳(旧 daemon)
        // 排尾,彼此按 device_id。排序与 TS deviceColorMap 逐语义镜像。
        let devices = [dev("z", nil), dev("b", 30), dev("a", 10), dev("m", 30), dev("a2", nil)]
        let map = FavoritesPalette.colorMap(devices: devices)
        // 位次:a(10)→0;并列 30 按 id b→1、m→2;缺戳按 id a2→3、z→4
        XCTAssertEqual(map["a"], FavoritesPalette.hexes[0], "created_at 10 最先")
        XCTAssertEqual(map["b"], FavoritesPalette.hexes[1], "created_at 30 并列,device_id b<m")
        XCTAssertEqual(map["m"], FavoritesPalette.hexes[2])
        XCTAssertEqual(map["a2"], FavoritesPalette.hexes[3], "缺戳排尾,device_id a2<z")
        XCTAssertEqual(map["z"], FavoritesPalette.hexes[4])
        XCTAssertEqual(map.count, 5)
        // 空表 → 空 map
        XCTAssertTrue(FavoritesPalette.colorMap(devices: []).isEmpty)

        // >64 台回绕:第 65 台 = 第 1 色(shuffle 证排序键唯一时与输入序无关)
        var many = (0..<65).map { dev("d\($0)", Int64($0)) }
        many.shuffle()
        let bigMap = FavoritesPalette.colorMap(devices: many)
        XCTAssertEqual(bigMap["d0"], FavoritesPalette.hexes[0])
        XCTAssertEqual(bigMap["d64"], FavoritesPalette.hexes[0], "第 65 台回绕=第 1 色")
        XCTAssertEqual(bigMap["d32"], FavoritesPalette.hexes[32])
    }

    // MARK: ⑦rename/removeMember/delete 只动目标条目

    func testEditsTouchOnlyTargetEntry() async throws {
        let fake = FakeDaemon()
        let script = KvScript()
        fake.onLine = KvScript.handler(script)
        let f1 = fav("f1", "旧名", members: [FavMember(device: "d1", path: "C:\\a")])
        let f2 = Favorite(id: "f2", name: "其他", createdAt: 7, members: [FavMember(device: "d9", path: "X:\\y")])
        script.set(FavoritesStore.kvKey, seedDoc([f1, f2]))
        let (store, repo) = makeStore(fake)
        await store.refresh(repo: repo)

        // rename:只改目标名;created_at 不动(纯函数面),updated_at 盖新戳
        let renamed = await store.rename(repo: repo, id: "f1", to: "新名")
        XCTAssertTrue(renamed)
        let put1 = try decodePut(script.putValues[0])
        XCTAssertEqual(put1.favorites[0].name, "新名")
        XCTAssertEqual(put1.favorites[0].createdAt, t0, "rename 纯函数不动 created_at")
        XCTAssertEqual(put1.favorites[1], f2, "未点名条目逐字不动")
        XCTAssertGreaterThan(put1.updatedAt, t0, "store 统一盖新戳")

        // removeMember:只删目标内匹配 (device,path) 的那一项
        let removed = await store.removeMember(repo: repo, id: "f1", member: FavMember(device: "d1", path: "C:\\a"))
        XCTAssertTrue(removed)
        let put2 = try decodePut(script.putValues[1])
        XCTAssertTrue(put2.favorites[0].members.isEmpty)
        XCTAssertEqual(put2.favorites[1].members, f2.members, "未点名条目成员不动")

        // delete=解除链接:只删目标条目,绝不触碰其他条目
        let deleted = await store.delete(repo: repo, id: "f2")
        XCTAssertTrue(deleted)
        let put3 = try decodePut(script.putValues[2])
        XCTAssertEqual(put3.favorites.map(\.id), ["f1"])
        XCTAssertEqual(store.favorites.map(\.id), ["f1"], "put 成功才落状态")
        XCTAssertFalse(store.busy)
    }

    // MARK: 补:busy 重入门(在途编辑拒新编辑)

    func testBusyGateRejectsReentrantEdit() async throws {
        let fake = FakeDaemon()
        let script = KvScript()
        fake.onLine = KvScript.handler(script)
        script.set(FavoritesStore.kvKey, seedDoc([fav("f1", "旧值")]))
        script.getDelaySeconds = 0.3 // kv.get 延迟应答:首个编辑稳定在途
        let (store, repo) = makeStore(fake)
        await store.refresh(repo: repo)

        let task = Task { await store.create(repo: repo, name: "并发新建") }
        let deadline = Date().addingTimeInterval(5)
        while script.kvGetCount < 2 && Date() < deadline {
            try await Task.sleep(nanoseconds: 25_000_000)
        }
        XCTAssertEqual(script.kvGetCount, 2, "编辑前置取已发出")
        XCTAssertTrue(store.busy, "在途编辑 busy 上帧")

        let second = await store.create(repo: repo, name: "重入拒")
        XCTAssertFalse(second, "busy 门拒重入")
        XCTAssertEqual(script.kvGetCount, 2, "重入不得再发 kv.get")
        XCTAssertEqual(script.kvPutCount, 0)

        let ok = await task.value
        XCTAssertTrue(ok)
        XCTAssertEqual(script.kvPutCount, 1)
        XCTAssertFalse(store.busy, "完成 busy 复位")
        XCTAssertEqual(store.favorites.count, 2, "put 成功才落状态")
    }
}
