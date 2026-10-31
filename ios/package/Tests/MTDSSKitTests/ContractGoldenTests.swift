import XCTest
import MTDSSKit

/// IPC 契约 golden 双端锁定:crates/ipc/contract 目录下 *.json(daemon 契约
/// 测试同源,单一真源)逐一对齐 Swift 客户端(镜像 android ContractGoldenTest)——
///
/// - `{op}.req.json`:类型化调用经伪 server 走完整客户端路径,线上请求帧与
///   golden 文件**逐字节**比对(键序 = serde_json 字节序,含尾换行);
/// - `{op}.resp.json`:类型化解析吃下 golden 应答并核对字段值(字段名清单
///   内嵌于断言——任何字段改名/缺失/漂移即红);
/// - `{event}.json`:事件帧经事件流投递并类型化解析核对;
/// - 全量文件 JSON 解析 + Codable 往返等价 + **孤儿 golden 防护**:目录里出现
///   客户端面未绑定、也未声明不移植/平台变体的新形态 → 立即红。
final class ContractGoldenTests: XCTestCase {

    /// 客户端面不移植的 req 形态(spec §4.2 便携面之外;golden 仅解析校验)。
    private let notPortedForms: Set<String> = [
        "set_recv_dir",
        "transfer.start.local_copy",
        "transfer.start.remote_copy",
    ]

    /// 平台变体 golden(T6 安卓收件箱哨兵投影;iOS 收件箱 = App Documents 绝对
    /// 路径,spec §5.2,不走哨兵——解析校验即可,不绑定)。
    private let platformVariants: Set<String> = [
        "status.resp.android.json",
        "set_recv_dir.resp.android.json",
    ]

    /// 事件 golden 文件 → 期望断言。
    private lazy var eventChecks: [String: (IpcEvent) -> Void] = [
        "event.login.json": { e in
            XCTAssertEqual(e, .login(loggedIn: true), "event.login 解析漂移")
        },
        "event.online.json": { e in
            XCTAssertEqual(e, .online(online: true), "event.online 解析漂移")
        },
        "event.devices.json": { e in
            guard case .devices(let devices) = e else { return XCTFail("非 devices 事件") }
            XCTAssertEqual(devices.count, 2)
            XCTAssertEqual(devices[0].deviceId, "10ba682c8ad13513971e8b56881aab8b")
            XCTAssertEqual(devices[0].name, "契约本机")
            XCTAssertEqual(devices[0].platform, "windows")
            XCTAssertEqual(devices[1].deviceId, "dev-peer-fixed")
            XCTAssertEqual(devices[1].name, "契约对端")
            XCTAssertEqual(devices[1].platform, "linux")
        },
        "event.transfer.json": { e in
            guard case .transfer(let p) = e else { return XCTFail("非 transfer 事件") }
            XCTAssertEqual(p.id, "3")
            XCTAssertEqual(p.kind, "upload")
            XCTAssertEqual(p.state, "queued")
            XCTAssertEqual(p.bytesDone, 0)
            XCTAssertEqual(p.bytesTotal, 4096)
            XCTAssertEqual(p.filesDone, 0)
            XCTAssertEqual(p.filesTotal, 1)
            XCTAssertEqual(p.skipped, 0)
            XCTAssertEqual(p.speedBps, 0)
            XCTAssertFalse(p.relay)
            XCTAssertNil(p.error)
        },
        "event.notify.json": { e in
            XCTAssertEqual(e, .notify(title: "MTDSS", body: "«契约对端» 向 /mtdss/contract-inbox 写入 2 个文件"))
        },
    ]

    /// 客户端面全形态注册表:golden 形态名 → 类型化调用 + 结果断言
    /// (镜像 android ContractGoldenTest 的 forms;方法名/字段值逐一锁定)。
    private lazy var forms: [String: (MtdssRepository) async throws -> Void] = [
        "hello": { r in
            let v = try await r.hello()
            XCTAssertEqual(v.version, "1.1.0")
            XCTAssertTrue(v.loggedIn)
            XCTAssertEqual(v.device?.deviceId, "10ba682c8ad13513971e8b56881aab8b")
            XCTAssertNil(v.device?.name)
        },
        "status": { r in
            let v = try await r.status()
            XCTAssertTrue(v.loggedIn)
            XCTAssertFalse(v.online)
            XCTAssertEqual(v.server, "http://127.0.0.1:39731")
            XCTAssertEqual(v.recvDir, "/mtdss/contract-inbox")
            XCTAssertEqual(v.device?.deviceId, "10ba682c8ad13513971e8b56881aab8b")
        },
        "devices": { r in
            let v = try await r.devices()
            XCTAssertFalse(v.stale)
            XCTAssertEqual(v.devices.count, 2)
            XCTAssertEqual(v.devices[0].deviceId, "10ba682c8ad13513971e8b56881aab8b")
            XCTAssertEqual(v.devices[0].name, "契约本机")
            XCTAssertEqual(v.devices[0].platform, "windows")
            XCTAssertFalse(v.devices[0].online)
            XCTAssertEqual(v.devices[1].deviceId, "dev-peer-fixed")
            XCTAssertEqual(v.devices[1].name, "契约对端")
            XCTAssertEqual(v.devices[1].platform, "linux")
        },
        "login.request_code": { r in
            let v = try await r.loginRequestCode(server: "http://127.0.0.1:39731", email: "contract@example.com")
            XCTAssertEqual(v.code, "654321")
        },
        "login.verify": { r in
            let v = try await r.loginVerify(code: "654321", email: "contract@example.com", name: "契约本机")
            XCTAssertEqual(v.deviceId, "10ba682c8ad13513971e8b56881aab8b")
            XCTAssertEqual(v.name, "契约本机")
        },
        "login.logout": { r in
            try await r.loginLogout()
        },
        "list": { r in
            await self.assertRpcError(3, "设备离线: dev-peer-fixed", kind: nil) {
                try await r.list(device: "dev-peer-fixed", path: "/远端/文档")
            }
        },
        "stat": { r in
            await self.assertRpcError(3, "设备离线: dev-peer-fixed", kind: nil) {
                try await r.stat(device: "dev-peer-fixed", path: "/远端/契约 文档.pdf")
            }
        },
        "transfer.start.upload": { r in
            let v = try await r.transferStartUpload(
                files: ["contract-fixture/契约样本.bin"],
                to: "dev-peer-fixed",
                dir: "/golden/peer-inbox",
            )
            XCTAssertEqual(v.jobId, "3")
        },
        "transfer.start.upload-bridge": { r in
            let v = try await r.transferStartUploadBridge(
                items: [BridgeItem(ref: "bridge-sample-ref", rel: "契约桥接样本.bin", size: 4096, mtime: 1_760_000_000_000)],
                to: "dev-peer-fixed",
                dir: "/golden/peer-inbox",
            )
            XCTAssertEqual(v.jobId, "6")
        },
        "transfer.start.download": { r in
            await self.assertRpcError(1, "dir 非法(须为绝对路径,Task 1 口径): golden/inbox") {
                try await r.transferStartDownload(
                    device: "dev-peer-fixed",
                    paths: ["/远端/契约 文档.pdf"],
                    dir: "golden/inbox",
                )
            }
        },
        "transfer.start.download-inbox": { r in
            await self.assertRpcError(1, "dir 非法(须为绝对路径,Task 1 口径): inbox") {
                // 收件箱哨兵:同一 builder,dir 取哨兵值
                try await r.transferStartDownload(
                    device: "dev-peer-fixed",
                    paths: ["/远端/契约 文档.pdf"],
                    dir: "inbox",
                )
            }
        },
        "transfer.list": { r in
            let v = try await r.transferList()
            XCTAssertEqual(v.jobs.count, 3)
            let j1 = v.jobs[0]
            XCTAssertEqual(j1.id, "1")
            XCTAssertEqual(j1.kind, "upload")
            XCTAssertEqual(j1.name, "契约样本.bin")
            XCTAssertEqual(j1.state, "failed")
            XCTAssertEqual(j1.filesDone, 0)
            XCTAssertEqual(j1.filesTotal, 1)
            XCTAssertEqual(j1.bytesDone, 0)
            XCTAssertEqual(j1.bytesTotal, 4096)
            XCTAssertEqual(j1.speedBps, 0)
            XCTAssertEqual(j1.skipped, 0)
            XCTAssertFalse(j1.relay)
            XCTAssertEqual(j1.src, "contract-fixture/契约样本.bin")
            XCTAssertEqual(j1.dst, "dev-peer-fixed:/golden/peer-inbox")
            XCTAssertEqual(j1.error, "发送失败(共尝试 6 轮,最后错误: 信令未在线,无法发起发送)")
            let j2 = v.jobs[1]
            XCTAssertEqual(j2.id, "2")
            XCTAssertEqual(j2.kind, "download")
            XCTAssertEqual(j2.state, "cancelled")
            XCTAssertEqual(j2.src, "dev-peer-fixed:/远端/契约 文档.pdf")
            XCTAssertEqual(j2.dst, "/golden/inbox")
            XCTAssertNil(j2.error)
            let j3 = v.jobs[2]
            XCTAssertEqual(j3.id, "3")
            XCTAssertEqual(j3.state, "cancelled")
            XCTAssertEqual(j3.speedBps, 0)
            XCTAssertFalse(j3.relay)
        },
        "transfer.pause": { r in
            try await r.transferPause(id: "3")
        },
        "transfer.resume": { r in
            try await r.transferResume(id: "3")
        },
        "transfer.cancel": { r in
            try await r.transferCancel(id: "3")
        },
        "transfer.wait": { r in
            let v = try await r.transferWait(id: "3")
            XCTAssertEqual(v.state, "cancelled")
            XCTAssertNil(v.error)
            XCTAssertEqual(v.filesDone, 0)
            XCTAssertEqual(v.filesTotal, 1)
            XCTAssertEqual(v.bytesTotal, 4096)
            XCTAssertEqual(v.skipped, 0)
        },
        "transfer.reinitiate": { r in
            try await r.transferReinitiate(id: "1")
        },
        "transfer.clear": { r in
            try await r.transferClear(id: "2")
        },
    ]

    // ── 测试 ────────────────────────────────────────────────────────────────

    /// 全部 req 形态:请求帧逐字节对齐 golden 且应答类型化解析吃下 golden。
    func testAllReqFormsByteExactWithTypedResponses() async throws {
        let reqNames = Goldens.allNames().filter { $0.hasSuffix(".req.json") }
        XCTAssertFalse(reqNames.isEmpty, "golden 目录为空")

        // 孤儿 golden 防护:未绑定且未声明不移植的新形态 → 红
        let unbound = reqNames
            .map { String($0.dropLast(".req.json".count)) }
            .filter { forms[$0] == nil && !notPortedForms.contains($0) }
        XCTAssertTrue(unbound.isEmpty, "golden 出现未绑定形态(客户端面缺绑定,或需声明不移植): \(unbound)")

        for name in reqNames.sorted() {
            let form = String(name.dropLast(".req.json".count))
            guard let check = forms[form] else {
                continue // 不移植形态:解析校验在「全量文件无遗漏」用例
            }
            let (fake, client, repo) = try await makeClient(respName: "\(form).resp.json")
            do {
                try await check(repo)
                let connections = fake.connections
                XCTAssertEqual(connections.count, 1, "\(form) 恰好一条连接")
                XCTAssertEqual(connections.first?.received.count, 1, "\(form) 恰好发出一个请求")
                // 字节级请求帧比对:golden 文件内容 = 线上帧(含尾换行)
                XCTAssertEqual(connections.first?.received.first, Goldens.read(name), "\(form) 请求帧漂移")
            } catch {
                XCTFail("形态 \(form) 失败: \(error)")
            }
            await client.close()
        }
    }

    /// 事件 golden 全量:事件流投递且类型化解析核对。
    func testEventGoldenDeliveredTyped() async throws {
        for (name, check) in eventChecks {
            let fake = FakeDaemon()
            let client = IpcClient(endpoint: fake)
            let collector = EventCollector(client.events) // 订阅先于连接:事件流带缓冲,无丢帧竞态
            try await client.connect()
            fake.connections.first?.sendLine(Goldens.read(name)) // 原样字节 = 线上事件帧
            let events = await collector.waitCount(1)
            guard let first = events.first else {
                XCTFail("事件 \(name) 未投递")
                await client.close()
                continue
            }
            check(first)
            await client.close()
        }
    }

    /// 全量 golden 文件解析校验 + Codable 往返等价 + 无孤儿遗漏。
    func testAllGoldenFilesParseCodableRoundTripAndNoOrphans() throws {
        let all = Goldens.allNames()
        XCTAssertFalse(all.isEmpty)
        for name in all {
            let frame = try JSONParser.parse(Goldens.text(name))
            guard case .object = frame else {
                return XCTFail("\(name) 须为 JSON 对象")
            }
            // Codable 往返等价(JSONValue → JSONEncoder → JSONDecoder → 等值)
            let data = try JSONEncoder().encode(frame)
            let decoded = try JSONDecoder().decode(JSONValue.self, from: data)
            XCTAssertEqual(decoded, frame, "\(name) Codable 往返漂移")
        }

        // 消费面:客户端形态 req/resp + 平台变体 + 事件 + 不移植形态 req/resp
        var consumed = Set<String>()
        for form in forms.keys {
            consumed.insert("\(form).req.json")
            consumed.insert("\(form).resp.json")
        }
        consumed.formUnion(platformVariants)
        consumed.formUnion(eventChecks.keys)
        for form in notPortedForms {
            consumed.insert("\(form).req.json")
            consumed.insert("\(form).resp.json")
        }
        let leftover = all.filter { !consumed.contains($0) }
        XCTAssertTrue(leftover.isEmpty, "golden 目录出现测试未消费的新文件(孤儿 golden 盲区防护): \(leftover)")
        // 反向:声明的消费面文件必须真实存在(防声明与目录脱节)
        for name in consumed {
            XCTAssertTrue(all.contains(name), "声明的消费面文件缺失: \(name)")
        }
    }

    /// 规范写出与 serde_json 字节口径一致(键序与转义)。
    func testCanonicalWriteMatchesSerdeJsonBytes() {
        // 键按 UTF-8 字节序(含 CJK 键)+ 转义:仅 " \ 与 <0x20 控制字符
        // (\b\f\n\r\t 快捷式,其余 \u00xx 小写十六进制);非 ASCII 原样直出。
        // 值 "a" = 引号、反斜杠、换行、0x01 四个字符的转义面。
        let tricky = "\"\\\n\u{01}"
        let value = JSONValue.object([
            "b": .array([.uint(1), .number("2.5"), .null, .bool(true)]),
            "a": .string(tricky),
            "中": .string("文"),
        ])
        XCTAssertEqual(
            value.canonical,
            "{\"a\":\"\\\"\\\\\\n\\u0001\",\"b\":[1,2.5,null,true],\"中\":\"文\"}",
        )
    }

    // ── 助手 ────────────────────────────────────────────────────────────────

    /// 新客户端 + 伪 server(应答指定 golden 帧,id 重写为请求 id)。
    private func makeClient(respName: String) async throws -> (FakeDaemon, IpcClient, MtdssRepository) {
        let fake = FakeDaemon()
        fake.onLine = { conn, line in
            let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self))
            guard case .number(let raw)? = frame?["id"], let id = UInt64(raw) else { return }
            conn.sendLine(Goldens.withId(Goldens.read(respName), id))
        }
        let client = IpcClient(endpoint: fake)
        try await client.connect()
        return (fake, client, MtdssRepository(client: client))
    }

    /// 断言调用抛 RpcError 并核对 code/message/kind。
    private func assertRpcError(
        _ code: Int64,
        _ message: String,
        kind: String? = nil,
        _ run: () async throws -> Void,
    ) async {
        do {
            try await run()
            XCTFail("应抛 RpcError(code: \(code))")
        } catch let e as RpcError {
            XCTAssertEqual(e.code, code)
            XCTAssertEqual(e.message, message)
            XCTAssertEqual(e.kind, kind)
        } catch {
            XCTFail("非 RpcError: \(error)")
        }
    }
}
