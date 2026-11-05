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
    /// T6 起 remote_copy 移出(转发面绑定,见 forms);local_copy 仍不移植
    /// (同机回迁是桌面粘贴面,iOS 无此流程)。
    private let notPortedForms: Set<String> = [
        "set_recv_dir",
        "transfer.start.local_copy",
    ]

    /// 平台变体 golden(T6 安卓收件箱哨兵投影;iOS 收件箱 = App Documents 绝对
    /// 路径,spec §5.2,不走哨兵——解析校验即可,不绑定)。
    private let platformVariants: Set<String> = [
        "status.resp.android.json",
        "set_recv_dir.resp.android.json",
    ]

    /// 形态样本 golden(v2-P5 T5b:list resp 条目形状,children 可选面)。
    private let shapeVariants: Set<String> = ["list.resp.entries.json"]

    /// 登录步进变体 golden(V3-1:verify approval/limit 投影 + qr_status
    /// pending + kv not_found typed 错误;typed 解析在专项用例,forms 只锁
    /// 主形态)。
    private let loginStepVariants: Set<String> = [
        "login.verify.resp.approval.json",
        "login.verify.resp.limit.json",
        "login.qr_status.resp.pending.json",
        "kv.get.resp.notfound.json",
    ]

    /// 扫码授权变体 golden(Ruling R8:claim 死码 typed not_found 错误 +
    /// approve 单次裁决重放/过期的 expired 投影;专项用例)。
    private let qrApprovalVariants: Set<String> = [
        "qr.claim.resp.expired.json",
        "qr.approve.resp.expired.json",
    ]

    /// request_code echo_off 变体 golden(final review C1:产线邮件直投回
    /// `{}` 无回显,code 容缺省空串;专项用例)。
    private let requestCodeVariants: Set<String> = [
        "login.request_code.resp.echo_off.json",
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
        "event.approval.json": { e in
            guard case .approval(let sessionId, let name, let platform, let code, let expiresAt) = e else {
                return XCTFail("非 approval 事件")
            }
            XCTAssertEqual(sessionId, "sess-contract-a")
            XCTAssertEqual(name, "契约申请端")
            XCTAssertEqual(platform, "windows")
            XCTAssertEqual(code, "123456")
            XCTAssertEqual(expiresAt, 1_760_000_200)
        },
        "event.device_evicted.json": { e in
            XCTAssertEqual(e, .deviceEvicted, "event.device_evicted 解析漂移")
        },
        "event.devices_changed.json": { e in
            XCTAssertEqual(e, .devicesChanged, "event.devices_changed 解析漂移")
        },
    ]

    /// 客户端面全形态注册表:golden 形态名 → 类型化调用 + 结果断言
    /// (镜像 android ContractGoldenTest 的 forms;方法名/字段值逐一锁定)。
    private lazy var forms: [String: (MtdssRepository) async throws -> Void] = [
        "hello": { r in
            let v = try await r.hello()
            XCTAssertEqual(v.version, "1.2.0")
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
            // V3-1:verify 应答 = 步进面(ok 收口投影)
            let step = try await r.loginVerify(code: "654321", email: "contract@example.com", name: "契约本机")
            guard case .ok(let v) = step else { return XCTFail("应答应落 ok 步进: \(step)") }
            XCTAssertEqual(v.deviceId, "10ba682c8ad13513971e8b56881aab8b")
            XCTAssertEqual(v.name, "契约本机")
        },
        "login.submit_code": { r in
            let step = try await r.loginSubmitCode(sessionId: "sess-contract-a", code: "123456")
            guard case .ok(let v) = step else { return XCTFail("应答应落 ok 步进: \(step)") }
            XCTAssertEqual(v.deviceId, "10ba682c8ad13513971e8b56881aab8b")
            XCTAssertEqual(v.name, "契约本机")
        },
        "login.status": { r in
            let step = try await r.loginStatus(sessionId: "sess-contract-b")
            guard case .ok(let v) = step else { return XCTFail("应答应落 ok 步进: \(step)") }
            XCTAssertEqual(v.name, "契约本机")
        },
        "login.evict": { r in
            let step = try await r.loginEvict(enrollToken: "enroll-contract-fixed", deviceId: "dev-peer-fixed")
            guard case .ok(let v) = step else { return XCTFail("应答应落 ok 步进: \(step)") }
            XCTAssertEqual(v.name, "契约本机")
        },
        "login.cancel": { r in
            try await r.loginCancel()
        },
        "login.qr_begin": { r in
            // 扫码面(V3-1 仅契约绑定,UI 随 V3-4)
            let v = try await r.loginQrBegin(server: "http://127.0.0.1:39731", name: "契约本机")
            XCTAssertEqual(v.qrUrl, "https://www.mtdss.top/qr?t=qrt-contract-fixed")
            XCTAssertEqual(v.token, "qrt-contract-fixed")
            XCTAssertEqual(v.expiresAt, 1_760_000_300)
        },
        "login.qr_status": { r in
            let step = try await r.loginQrStatus(token: "qrt-contract-fixed")
            guard case .ok = step else { return XCTFail("应答应落 ok 步进: \(step)") }
        },
        "approval.respond": { r in
            try await r.approvalRespond(sessionId: "sess-contract-a", allow: true)
        },
        "qr.claim": { r in
            // 扫码授权闭环(Ruling R8 手机侧透传;候选面拍平)
            let v = try await r.qrClaim(token: "qrt-contract-fixed")
            XCTAssertEqual(v.candidateName, "契约新机")
            XCTAssertEqual(v.candidatePlatform, "windows")
            XCTAssertEqual(v.expiresAt, 1_760_000_300)
        },
        "qr.approve": { r in
            let v = try await r.qrApprove(token: "qrt-contract-fixed", allow: true)
            XCTAssertEqual(v.status, "ok")
        },
        "kv.get": { r in
            let v = try await r.kvGet(key: "常用目录")
            XCTAssertEqual(v, "{\"dirs\":[\"/工作\"]}")
        },
        "kv.put": { r in
            let ok = try await r.kvPut(key: "常用目录", value: "{\"dirs\":[\"/工作\"]}")
            XCTAssertTrue(ok)
        },
        "login.logout": { r in
            try await r.loginLogout()
        },
        "list": { r in
            await self.assertRpcError(3, "设备离线: dev-peer-fixed", kind: nil) {
                _ = try await r.list(device: "dev-peer-fixed", path: "/远端/文档")
            }
        },
        "stat": { r in
            await self.assertRpcError(3, "设备离线: dev-peer-fixed", kind: nil) {
                _ = try await r.stat(device: "dev-peer-fixed", path: "/远端/契约 文档.pdf")
            }
        },
        "delete": { r in
            // T5b ③ 入锚:参数面 {device, paths, force} 请求帧逐字节对齐
            // golden(本机删除动作 BrowseViewModel.deleteSelected 同一门面)。
            await self.assertRpcError(3, "设备离线: dev-peer-fixed", kind: nil) {
                _ = try await r.delete(
                    device: "dev-peer-fixed",
                    paths: ["/远端/契约 文档.pdf"],
                    force: false,
                )
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
                _ = try await r.transferStartDownload(
                    device: "dev-peer-fixed",
                    paths: ["/远端/契约 文档.pdf"],
                    dir: "golden/inbox",
                )
            }
        },
        "transfer.start.download-inbox": { r in
            await self.assertRpcError(1, "dir 非法(须为绝对路径,Task 1 口径): inbox") {
                // 收件箱哨兵:同一 builder,dir 取哨兵值
                _ = try await r.transferStartDownload(
                    device: "dev-peer-fixed",
                    paths: ["/远端/契约 文档.pdf"],
                    dir: "inbox",
                )
            }
        },
        "transfer.start.remote_copy": { r in
            // T6 转发面:远端 A → 远端 B 调度(daemon 编排,数据不经操作端)
            let v = try await r.transferStartRemoteCopy(
                from: "dev-peer-fixed",
                paths: ["/远端/直传 目录"],
                to: "dev-peer2-fixed",
                dir: "/golden/peer2-inbox",
            )
            XCTAssertEqual(v.jobId, "10ba682c8ad13513971e8b56881aab8b:5")
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

        // 消费面:客户端形态 req/resp + 平台变体 + 形态样本 + 登录步进变体
        // + 扫码授权变体 + 事件 + 不移植形态 req/resp
        var consumed = Set<String>()
        for form in forms.keys {
            consumed.insert("\(form).req.json")
            consumed.insert("\(form).resp.json")
        }
        consumed.formUnion(platformVariants)
        consumed.formUnion(shapeVariants)
        consumed.formUnion(loginStepVariants)
        consumed.formUnion(qrApprovalVariants)
        consumed.formUnion(requestCodeVariants)
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

    /// list 条目形态样本:DirEntry children 可选面(v2-P5 T5b)——目录项
    /// 携直接子项数,文件项整键缺省 → nil;副文接线(「日期 - N 项」)。
    func testListEntriesShapeVariantParsesChildren() throws {
        let frame = try JSONParser.parse(Goldens.text("list.resp.entries.json"))
        guard case .object(let obj) = frame, case .object(let result)? = obj["result"] else {
            return XCTFail("list.resp.entries.json 须为应答帧")
        }
        let v = try ListResult(.object(result))
        XCTAssertFalse(v.truncated)
        XCTAssertEqual(v.entries.count, 2)
        let dir = v.entries[0]
        XCTAssertEqual(dir.name, "文档 目录")
        XCTAssertTrue(dir.isDir)
        XCTAssertEqual(dir.children, 2, "目录项携直接子项数")
        let file = v.entries[1]
        XCTAssertEqual(file.name, "契约 文档.pdf")
        XCTAssertFalse(file.isDir)
        XCTAssertNil(file.children, "文件项无 children 键 → nil")
        // 副文接线(BrowseView 行渲染同表达式):目录「日期 - N 项」。
        XCTAssertEqual(
            BrowseFormat.subtitle(dir, childCount: dir.children.map { Int($0) }),
            "\(BrowseFormat.date(dir.mtimeMs)) - 2 项",
        )
        // 缺省构造(文件项/stat 单查面):children 恒 nil(退化为纯日期)。
        let bare = DirEntry(name: "x", path: "/x", size: 0, mtimeMs: 0, isDir: true)
        XCTAssertNil(bare.children)
        XCTAssertEqual(BrowseFormat.subtitle(bare), "", "mtime 0 且数未知 → 纯空副文")
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

    /// 登录步进变体——approval/limit/pending 投影类型化解析(V3-1)。
    /// verify/qr_status 的非 ok 应答(status 判别面中间态)typed 步进逐字段
    /// 吃下 golden;主形态(req/resp 主对)由 forms 锁。
    func testLoginStepVariantsTypedParse() async throws {
        let cases: [String: () async throws -> LoginStep] = [
            "login.verify.resp.approval.json": { [self] in
                try await makeRepoCall(respName: "login.verify.resp.approval.json") {
                    try await $0.loginVerify(code: "654321", email: "contract@example.com", name: "契约本机")
                }
            },
            "login.verify.resp.limit.json": { [self] in
                try await makeRepoCall(respName: "login.verify.resp.limit.json") {
                    try await $0.loginVerify(code: "654321", email: "contract@example.com", name: "契约本机")
                }
            },
            "login.qr_status.resp.pending.json": { [self] in
                try await makeRepoCall(respName: "login.qr_status.resp.pending.json") {
                    try await $0.loginQrStatus(token: "qrt-contract-fixed")
                }
            },
        ]
        for (name, call) in cases {
            do {
                let step = try await call()
                switch (name, step) {
                case ("login.verify.resp.approval.json", .approval(let sessionId, let expiresAt)):
                    XCTAssertEqual(sessionId, "sess-contract-a", name)
                    XCTAssertEqual(expiresAt, 1_760_000_200, name)
                case ("login.verify.resp.limit.json", .limit(let token, let devices)):
                    XCTAssertEqual(token, "enroll-contract-fixed", name)
                    XCTAssertEqual(devices.count, 2, name)
                    XCTAssertEqual(devices[0].name, "契约本机", name)
                    XCTAssertEqual(devices[0].platform, "windows", name)
                    XCTAssertEqual(devices[1].name, "契约对端", name)
                    XCTAssertEqual(devices[1].platform, "linux", name)
                case ("login.qr_status.resp.pending.json", .pending(let expiresAt)):
                    XCTAssertEqual(expiresAt, 1_760_000_300, name)
                default:
                    XCTFail("\(name) 步进形态漂移: \(step)")
                }
            } catch {
                XCTFail("登录步进变体 \(name) 失败: \(error)")
            }
        }
    }

    /// kv.get 未命中形态——typed not_found 错误投影(V3-1;错误帧无 result,
    /// 非 forms 面)。请求帧字节锁在 forms 主形态(同 key 常用目录),此处
    /// 只锁错误投影(android 同口径)。
    func testKvGetNotFoundTypedError() async throws {
        let (_, client, repo) = try await makeClient(respName: "kv.get.resp.notfound.json")
        await assertRpcError(1, "无此键: 不存在键", kind: "not_found") {
            _ = try await repo.kvGet(key: "不存在键")
        }
        await client.close()
    }

    /// request_code echo_off 变体——产线无回显空对象容忍(final review C1)。
    /// 产线邮件直投模式 request_code 应答为 `{}`(码走邮件),code 必须容
    /// 缺省为空串(空 = 不自动预填),不得契约违规。主形态已由 forms 锁,
    /// 此处只锁变体帧(变体 req 不入库,android 同口径)。
    func testRequestCodeEchoOffVariantParsesEmptyCode() async throws {
        let (_, client, repo) = try await makeClient(respName: "login.request_code.resp.echo_off.json")
        let v = try await repo.loginRequestCode(server: "http://127.0.0.1:39731", email: "contract@example.com")
        XCTAssertEqual(v.code, "")
        await client.close()
    }

    /// 扫码授权变体(Ruling R8):claim 死码(404)→ typed not_found 错误
    /// 帧;approve 单次裁决重放/过期(409)→ expired 状态投影(非 RPC 错)。
    /// 主形态已由 forms 锁,此处只锁变体帧(变体 req 不入库,android 同口径)。
    func testQrApprovalVariantsTypedParse() async throws {
        let (_, claimClient, claimRepo) = try await makeClient(respName: "qr.claim.resp.expired.json")
        await assertRpcError(1, "二维码已失效,请重新扫码", kind: "not_found") {
            _ = try await claimRepo.qrClaim(token: "qrt-contract-dead")
        }
        await claimClient.close()

        let (_, approveClient, approveRepo) = try await makeClient(respName: "qr.approve.resp.expired.json")
        let v = try await approveRepo.qrApprove(token: "qrt-contract-dead", allow: true)
        XCTAssertEqual(v.status, "expired")
        await approveClient.close()
    }

    // ── 助手 ────────────────────────────────────────────────────────────────

    /// 单调用辅助:伪 server 以指定 golden 应答,跑一个 repo 调用取步进
    /// (变体用例不锁请求帧——主形态已在 forms 锁定)。
    private func makeRepoCall(
        respName: String,
        _ run: (MtdssRepository) async throws -> LoginStep,
    ) async throws -> LoginStep {
        let (_, client, repo) = try await makeClient(respName: respName)
        do {
            let step = try await run(repo)
            await client.close()
            return step
        } catch {
            await client.close()
            throw error
        }
    }

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
