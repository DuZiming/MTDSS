import MTDSSKit
import XCTest

/// 扫码授权闭环状态机单测(V3-1 Ruling R8,伪 daemon 脚本):
/// - claim:候选面 confirming 上帧(弹层数据源)/ 死码 typed not_found →
///   failed 文案 / RPC 失败 → failed;
/// - approve:同意/拒绝结果行文案(ok/denied/expired/limit 全映射)+ 请求
///   帧参数面(token/allow 逐字)+ 相位守卫(仅自 confirming 接受);
/// - dismiss 清场:迟到裁决不外发。
@MainActor
final class QrApprovalModelTests: XCTestCase {

    // MARK: 步骤脚本伪 daemon

    /// 按 method 应答:静态 result JSON 原文 / `@error:` 前缀 = 错误帧;
    /// 请求行留痕供帧形状断言(线程安全)。
    private final class StepScript: @unchecked Sendable {
        private let lock = NSLock()
        private var lines: [String] = []
        private var rules: [String: String] = [:]

        var received: [String] { lock.withLock { lines } }

        func rule(_ method: String, _ resultJSON: String) {
            lock.withLock { rules[method] = resultJSON }
        }

        static func handler(_ script: StepScript) -> FakeDaemon.LineHandler {
            { conn, line in
                let text = String(decoding: line, as: UTF8.self)
                script.lock.lock()
                script.lines.append(text)
                guard let frame = try? JSONParser.parse(text),
                      let method = frame["method"].flatMap({ try? $0.asString() }),
                      case .number(let idRaw)? = frame["id"] else {
                    script.lock.unlock()
                    return
                }
                let rule = script.rules[method]
                script.lock.unlock()
                let id = JSONValue.number(idRaw)
                if let rule, rule.hasPrefix("@error:") {
                    conn.sendLine(Self.error(id: id, message: String(rule.dropFirst("@error:".count))))
                } else if let rule {
                    conn.sendLine(Self.result(id: id, rawJSON: rule))
                } else {
                    conn.sendLine(Self.result(id: id, rawJSON: "{}"))
                }
            }
        }

        static func result(id: JSONValue, rawJSON: String) -> Data {
            let value = (try? JSONParser.parse(rawJSON)) ?? .object([:])
            return Data((JSONValue.object(["id": id, "result": value]).canonical + "\n").utf8)
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

    private static let claimOk =
        #"{"candidate_name":"契约新机","candidate_platform":"windows","expires_at":1760000300}"#

    private func makeModel(_ fake: FakeDaemon, timeout: TimeInterval = 15) -> QrApprovalModel {
        QrApprovalModel(repo: MtdssRepository(client: IpcClient(endpoint: fake)), callTimeout: timeout)
    }

    // MARK: claim(候选确认面)

    func testClaimFallsToConfirmingWithCandidate() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        script.rule(Ops.qrClaim, Self.claimOk)
        let model = makeModel(fake)

        await model.claim(token: "qrt-contract-fixed")
        guard case .some(.confirming(let c)) = model.stage else {
            return XCTFail("应落 confirming: \(String(describing: model.stage))")
        }
        XCTAssertEqual(c.candidateName, "契约新机")
        XCTAssertEqual(c.candidatePlatform, "windows")
        XCTAssertEqual(c.expiresAt, 1_760_000_300)
        XCTAssertEqual(model.token, "qrt-contract-fixed", "裁决随路 token 上帧")
        XCTAssertFalse(model.busy)

        // 请求参数面:token 逐字上帧
        let line = try XCTUnwrap(script.received.first { $0.contains(Ops.qrClaim) })
        XCTAssertTrue(line.contains("\"token\":\"qrt-contract-fixed\""), line)
    }

    func testClaimDeadTokenTypedErrorShowsFailedCopy() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        script.rule(Ops.qrClaim, "@error:二维码已失效,请重新扫码")
        let model = makeModel(fake)

        await model.claim(token: "qrt-contract-dead")
        guard case .some(.failed(let text)) = model.stage else {
            return XCTFail("应落 failed: \(String(describing: model.stage))")
        }
        XCTAssertEqual(text, "二维码已失效,请重新扫码", "daemon typed 文案直出")
        XCTAssertNil(model.token, "失败不记 token")
    }

    func testClaimTransportFailureShowsFailedCopy() async throws {
        let fake = FakeDaemon()
        // 静默吞掉 qr.claim(无应答)→ 注入小值超时
        fake.onLine = { _, _ in } // 吞帧
        let model = makeModel(fake, timeout: 0.2)

        await model.claim(token: "qrt-never-answered")
        guard case .some(.failed(let text)) = model.stage else {
            return XCTFail("应落 failed: \(String(describing: model.stage))")
        }
        XCTAssertEqual(text, "请求超时")
    }

    // MARK: approve(结果行)

    func testApproveAllowTrueShowsOkLineAndWireParams() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        script.rule(Ops.qrClaim, Self.claimOk)
        script.rule(Ops.qrApprove, #"{"status":"ok"}"#)
        let model = makeModel(fake)

        await model.claim(token: "qrt-contract-fixed")
        await model.approve(allow: true)
        guard case .some(.result(let line)) = model.stage else {
            return XCTFail("应落 result: \(String(describing: model.stage))")
        }
        XCTAssertEqual(line, "已同意扫码登录")
        XCTAssertFalse(model.busy)
        let line0 = try XCTUnwrap(script.received.first { $0.contains(Ops.qrApprove) })
        XCTAssertTrue(line0.contains("\"token\":\"qrt-contract-fixed\""), line0)
        XCTAssertTrue(line0.contains("\"allow\":true"), line0)
    }

    func testApproveAllowFalseShowsDeniedLineAndWireParams() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        script.rule(Ops.qrClaim, Self.claimOk)
        script.rule(Ops.qrApprove, #"{"status":"denied"}"#)
        let model = makeModel(fake)

        await model.claim(token: "qrt-contract-fixed")
        await model.approve(allow: false)
        guard case .some(.result(let line)) = model.stage else {
            return XCTFail("应落 result: \(String(describing: model.stage))")
        }
        XCTAssertEqual(line, "已拒绝该设备登录")
        let line0 = try XCTUnwrap(script.received.first { $0.contains(Ops.qrApprove) })
        XCTAssertTrue(line0.contains("\"allow\":false"), line0)
    }

    func testApproveExpiredAndLimitStatusLines() async throws {
        for (status, want) in [("expired", "二维码已过期,请重新扫码"), ("limit", "对方需选择移除设备后完成登录")] {
            let fake = FakeDaemon()
            let script = StepScript()
            fake.onLine = StepScript.handler(script)
            script.rule(Ops.qrClaim, Self.claimOk)
            script.rule(Ops.qrApprove, #"{"status":"\#(status)"}"#)
            let model = makeModel(fake)

            await model.claim(token: "qrt-contract-fixed")
            await model.approve(allow: true)
            guard case .some(.result(let line)) = model.stage else {
                return XCTFail("应落 result: \(String(describing: model.stage))")
            }
            XCTAssertEqual(line, want, status)
        }
    }

    func testApproveRpcErrorShowsFailedCopy() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        script.rule(Ops.qrClaim, Self.claimOk)
        script.rule(Ops.qrApprove, "@error:该二维码已由其他设备认领")
        let model = makeModel(fake)

        await model.claim(token: "qrt-contract-fixed")
        await model.approve(allow: true)
        guard case .some(.failed(let text)) = model.stage else {
            return XCTFail("应落 failed: \(String(describing: model.stage))")
        }
        XCTAssertEqual(text, "该二维码已由其他设备认领")
    }

    // MARK: 相位守卫 / 清场

    func testApproveWithoutConfirmingSendsNothing() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        script.rule(Ops.qrApprove, #"{"status":"ok"}"#)
        let model = makeModel(fake)

        // 初始态直接裁决:守卫拒,零外发
        await model.approve(allow: true)
        XCTAssertTrue(script.received.isEmpty, "非 confirming 相位不外发裁决")
    }

    func testDismissClearsStageAndLateApproveSendsNothing() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        script.rule(Ops.qrClaim, Self.claimOk)
        let model = makeModel(fake)

        await model.claim(token: "qrt-contract-fixed")
        guard case .some(.confirming) = model.stage else { return XCTFail("应落 confirming") }
        model.dismiss()
        XCTAssertNil(model.stage)
        XCTAssertNil(model.token, "清场后 token 撤帧")
        XCTAssertFalse(model.busy)

        // 迟到裁决(弹层已关):守卫拒,零外发
        await model.approve(allow: true)
        XCTAssertFalse(
            script.received.contains { $0.contains(Ops.qrApprove) },
            "dismiss 后迟到裁决不外发",
        )
    }

    func testResultLineMappingTable() {
        XCTAssertEqual(QrApprovalModel.resultLine(for: "ok"), "已同意扫码登录")
        XCTAssertEqual(QrApprovalModel.resultLine(for: "denied"), "已拒绝该设备登录")
        XCTAssertEqual(QrApprovalModel.resultLine(for: "expired"), "二维码已过期,请重新扫码")
        XCTAssertEqual(QrApprovalModel.resultLine(for: "limit"), "对方需选择移除设备后完成登录")
        XCTAssertEqual(QrApprovalModel.resultLine(for: "mystery"), "二维码状态异常")
    }
}
