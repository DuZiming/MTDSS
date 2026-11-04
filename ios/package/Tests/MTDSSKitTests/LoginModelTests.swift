import MTDSSKit
import XCTest

/// 登录四相位状态机单测(V3-1,先红后绿;伪 daemon 脚本;android
/// LoginViewModelTest 九状态机用例同象):
/// - verify 步进分派:approval(轮询收口 ok → Done)/ limit(直入踢选);
/// - 轮询:瞬态失败不断拍、denied/expired 终态回 Form 文案、撞 limit 转场;
/// - 审批码直输(空码本地校验 + 提交收口)、踢选收口(凭据上帧断言);
/// - 取消回退(停轮询 + login.cancel + 相位守卫拒迟到步进)。
@MainActor
final class LoginModelTests: XCTestCase {

    // MARK: 步进脚本伪 daemon

    /// 按 method 应答:静态 golden(result 原文)/ status 轮询序列(尾元素
    /// 重复;`@error:` 前缀 = 错误帧);请求行留痕供帧形状断言(线程安全)。
    private final class StepScript: @unchecked Sendable {
        private let lock = NSLock()
        private var lines: [String] = []
        /// method → result JSON 原文(静态应答)。
        private var rules: [String: String] = [:]
        /// status 轮询序列(逐拍消耗,尾元素重复;"" = error 帧)。
        private var statusSeq: [String] = []
        private var statusCalls = 0

        var received: [String] { lock.withLock { lines } }
        var statusCallCount: Int { lock.withLock { statusCalls } }

        func rule(_ method: String, _ resultJSON: String) {
            lock.withLock { rules[method] = resultJSON }
        }

        func statusSequence(_ results: [String]) {
            lock.withLock {
                statusSeq = results
                statusCalls = 0
            }
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
                script.lock.unlock()
                let id = JSONValue.number(idRaw)
                // status:轮询序列(逐拍;尾元素重复)
                if method == Ops.loginStatus {
                    let result = script.nextStatus()
                    if result.isEmpty {
                        conn.sendLine(Self.error(id: id, message: "信令抖动"))
                    } else {
                        conn.sendLine(Self.result(id: id, rawJSON: result))
                    }
                    return
                }
                if let rule = script.ruleFor(method) {
                    conn.sendLine(Self.result(id: id, rawJSON: rule))
                } else {
                    conn.sendLine(Self.result(id: id, rawJSON: "{}"))
                }
            }
        }

        private func nextStatus() -> String {
            lock.lock()
            defer { lock.unlock() }
            statusCalls += 1
            if statusSeq.isEmpty { return "{}" }
            return statusSeq[min(statusCalls - 1, statusSeq.count - 1)]
        }

        private func ruleFor(_ method: String) -> String? {
            lock.withLock { rules[method] }
        }

        static func result(id: JSONValue, rawJSON: String) -> Data {
            let value = (try? JSONParser.parse(rawJSON)) ?? .object([:])
            return Data((JSONValue.object(["id": id, "result": value]).canonical + "\n").utf8)
        }

        static func error(id: JSONValue, message: String) -> Data {
            Data((JSONValue.object(["id": id, "error": .object([
                "code": .number("1"),
                "message": .string(message),
            ])]).canonical + "\n").utf8)
        }
    }

    // MARK: 夹具

    private func makeModel(
        _ fake: FakeDaemon,
        pollDelay: TimeInterval = 0.05,
        onLogin: @escaping (LoginVerifyResult) -> Void = { _ in },
    ) -> (LoginModel, AccountStore) {
        let account = AccountStore(defaults: UserDefaults(suiteName: "v31-login-\(UUID().uuidString)")!)
        let login = LoginModel(
            repo: MtdssRepository(client: IpcClient(endpoint: fake)),
            account: account,
            defaultDeviceName: "iPhone",
            pollDelay: pollDelay,
            onLogin: onLogin,
        )
        return (login, account)
    }

    /// 填表 + verify(邮箱表单;回显码路径不参与状态机用例)。
    private func fillAndVerify(_ login: LoginModel) async {
        login.email = "kk@test.com"
        login.code = "654321"
        await login.verify()
    }

    private func awaitUntil(
        timeout: TimeInterval = 5,
        _ cond: @autoclosure () -> Bool,
    ) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while !cond() {
            if Date() > deadline { XCTFail("awaitUntil 超时") }
            try await Task.sleep(nanoseconds: 25_000_000)
        }
    }

    // MARK: verify 步进分派

    func testVerifyFallsToApprovalAndPollSettlesToDone() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        script.rule(Ops.loginVerify, #"{"status":"approval","session_id":"sess-contract-a","expires_at":1760000200}"#)
        script.statusSequence([
            #"{"status":"approval","session_id":"sess-contract-a","expires_at":1760000200}"#,
            #"{"status":"ok","device_id":"10ba682c8ad13513971e8b56881aab8b","name":"契约本机"}"#,
        ])
        let session = SessionModel()
        let (login, _) = makeModel(fake) { result in session.applyLogin(result) }

        await fillAndVerify(login)
        guard case .approval = login.phase else { return XCTFail("应落 Approval 相位: \(login.phase)") }
        XCTAssertEqual(login.sessionId, "sess-contract-a")
        XCTAssertEqual(login.expiresAt, 1_760_000_200)
        XCTAssertNil(login.succeeded, "审批挂起不是收口")

        // respond-settle:受信端应答后轮询领到 ok → Done(succeeded 只是屏内
        // 信号,导航权威态在 SessionModel)
        try await awaitUntil { login.phase == .done }
        XCTAssertGreaterThanOrEqual(script.statusCallCount, 2, "轮询至少两拍")
        let result = try XCTUnwrap(login.succeeded)
        XCTAssertEqual(result.deviceId, "10ba682c8ad13513971e8b56881aab8b")
        XCTAssertEqual(session.loggedIn, true, "onLogin → SessionModel 翻转")
        XCTAssertNil(login.errorText)
        XCTAssertFalse(login.busy)
    }

    func testVerifyFallsToLimitDirectly() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        script.rule(Ops.loginVerify, #"{"status":"limit","enroll_token":"enroll-contract-fixed","devices":[{"device_id":"10ba682c8ad13513971e8b56881aab8b","name":"契约本机","online":false,"platform":"windows"},{"device_id":"dev-peer-fixed","name":"契约对端","online":false,"platform":"linux"}]}"#)
        let (login, _) = makeModel(fake)

        await fillAndVerify(login)
        guard case .limit = login.phase else { return XCTFail("应落 Limit 相位: \(login.phase)") }
        XCTAssertEqual(login.enrollToken, "enroll-contract-fixed")
        XCTAssertEqual(login.evictDevices.count, 2)
        XCTAssertEqual(login.evictDevices[0].name, "契约本机")
        XCTAssertEqual(login.evictDevices[1].platform, "linux")
        XCTAssertNil(login.succeeded)
    }

    // MARK: 审批轮询

    func testPollTransientFailureKeepsPolling() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        script.rule(Ops.loginVerify, #"{"status":"approval","session_id":"sess-contract-a","expires_at":1760000200}"#)
        script.statusSequence([
            "", // 首拍错误帧(信令抖动)
            #"{"status":"ok","device_id":"dev-ios","name":"契约本机"}"#,
        ])
        let (login, _) = makeModel(fake)

        await fillAndVerify(login)
        try await awaitUntil { login.phase == .done }
        XCTAssertGreaterThanOrEqual(script.statusCallCount, 2, "首拍失败后仍续轮询收口")
        XCTAssertNotNil(login.succeeded)
    }

    func testPollDeniedTerminalReturnsToFormWithBanner() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        script.rule(Ops.loginVerify, #"{"status":"approval","session_id":"sess-contract-a","expires_at":1760000200}"#)
        script.statusSequence([#"{"status":"denied"}"#])
        let (login, _) = makeModel(fake)

        await fillAndVerify(login)
        // Approval 相位是瞬态(轮询 50ms 即落终态,逐帧断言必漏帧),直接等
        // 终态文案;Form 相位与中间态凭据清场由断言收口
        try await awaitUntil { login.terminal != nil }
        XCTAssertEqual(login.phase, .form)
        XCTAssertEqual(login.terminal, "登录请求已被拒绝")
        XCTAssertNil(login.sessionId)
        XCTAssertNil(login.succeeded)
    }

    func testPollExpiredTerminalReturnsToFormWithBanner() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        script.rule(Ops.loginVerify, #"{"status":"approval","session_id":"sess-contract-a","expires_at":1760000200}"#)
        script.statusSequence([#"{"status":"expired"}"#])
        let (login, _) = makeModel(fake)

        await fillAndVerify(login)
        try await awaitUntil { login.terminal != nil }
        XCTAssertEqual(login.phase, .form)
        XCTAssertEqual(login.terminal, "登录已超时,请重新发起")
        XCTAssertNil(login.succeeded)
    }

    func testPollHittingLimitTransitionsToEvictScreen() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        script.rule(Ops.loginVerify, #"{"status":"approval","session_id":"sess-contract-a","expires_at":1760000200}"#)
        script.statusSequence([#"{"status":"limit","enroll_token":"enroll-poll","devices":[{"device_id":"dev-peer-fixed","name":"契约对端","online":false,"platform":"linux"}]}"#])
        let (login, _) = makeModel(fake)

        await fillAndVerify(login)
        try await awaitUntil { login.phase == .limit }
        XCTAssertEqual(login.enrollToken, "enroll-poll")
        XCTAssertEqual(login.evictDevices.count, 1)
        XCTAssertEqual(login.evictDevices[0].name, "契约对端")
    }

    // MARK: 审批码直输 / 踢选收口

    func testSubmitApprovalCodeSettlesDone() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        script.rule(Ops.loginVerify, #"{"status":"approval","session_id":"sess-contract-a","expires_at":1760000200}"#)
        script.rule(Ops.loginSubmitCode, #"{"status":"ok","device_id":"dev-ios","name":"契约本机"}"#)
        let (login, _) = makeModel(fake)

        await fillAndVerify(login)
        guard case .approval = login.phase else { return XCTFail("应落 Approval 相位") }

        // 空码本地校验(不触 RPC)
        await login.submitApprovalCode()
        XCTAssertEqual(login.errorText, "请填写审批码")
        XCTAssertFalse(
            script.received.contains { $0.contains(Ops.loginSubmitCode) },
            "空码本地校验失败不触 RPC",
        )

        // 6 位审批码直输收口
        login.approvalCode = "123456"
        await login.submitApprovalCode()
        XCTAssertEqual(login.phase, .done)
        XCTAssertNotNil(login.succeeded)
        let submitLine = try XCTUnwrap(script.received.first { $0.contains(Ops.loginSubmitCode) })
        XCTAssertTrue(submitLine.contains("\"session_id\":\"sess-contract-a\""), submitLine)
        XCTAssertTrue(submitLine.contains("\"code\":\"123456\""), submitLine)
    }

    func testEvictSettlesDoneWithCredentialsOnWire() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        script.rule(Ops.loginVerify, #"{"status":"limit","enroll_token":"enroll-contract-fixed","devices":[{"device_id":"dev-peer-fixed","name":"契约对端","online":false,"platform":"linux"}]}"#)
        script.rule(Ops.loginEvict, #"{"status":"ok","device_id":"10ba682c8ad13513971e8b56881aab8b","name":"契约本机"}"#)
        let (login, _) = makeModel(fake)

        await fillAndVerify(login)
        guard case .limit = login.phase else { return XCTFail("应落 Limit 相位") }
        await login.evict(deviceId: "dev-peer-fixed")
        XCTAssertEqual(login.phase, .done)
        // 请求参数面:enroll_token + device_id 逐字对齐踢选凭据
        let evictLine = try XCTUnwrap(script.received.first { $0.contains(Ops.loginEvict) })
        XCTAssertTrue(evictLine.contains("\"enroll_token\":\"enroll-contract-fixed\""), evictLine)
        XCTAssertTrue(evictLine.contains("\"device_id\":\"dev-peer-fixed\""), evictLine)
    }

    func testEvictErrorStaysOnLimitScreen() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        script.rule(Ops.loginVerify, #"{"status":"limit","enroll_token":"enroll-contract-fixed","devices":[{"device_id":"dev-peer-fixed","name":"契约对端","online":false,"platform":"linux"}]}"#)
        // evict 未知错误(daemon 参数面拒)
        let handler = StepScript.handler(script)
        fake.onLine = { conn, line in
            let text = String(decoding: line, as: UTF8.self)
            if text.contains(Ops.loginEvict) {
                guard let frame = try? JSONParser.parse(text),
                      case .number(let idRaw)? = frame["id"] else { return }
                conn.sendLine(StepScript.error(id: .number(idRaw), message: "移出失败:设备不存在"))
                return
            }
            handler(conn, line)
        }
        let (login, _) = makeModel(fake)

        await fillAndVerify(login)
        await login.evict(deviceId: "dev-ghost")
        XCTAssertEqual(login.phase, .limit, "移出失败留在踢选屏")
        XCTAssertEqual(login.errorText, "移出失败:设备不存在")
        XCTAssertFalse(login.busy)
    }

    // MARK: 取消回退(相位守卫)

    func testCancelReturnsToFormStopsPollAndSendsCancelRPC() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        script.rule(Ops.loginVerify, #"{"status":"approval","session_id":"sess-contract-a","expires_at":1760000200}"#)
        script.statusSequence([#"{"status":"approval","session_id":"sess-contract-a","expires_at":1760000200}"#])
        let (login, _) = makeModel(fake)

        await fillAndVerify(login)
        try await awaitUntil { login.phase == .approval }
        login.cancel()
        XCTAssertEqual(login.phase, .form)
        XCTAssertNil(login.terminal)
        XCTAssertNil(login.sessionId)

        // 迟到的轮询步进不得把相位拖回 Approval(Form 期守卫)
        try await Task.sleep(nanoseconds: 300_000_000)
        XCTAssertEqual(login.phase, .form, "迟到轮询步进不得拖回旧相位")
        XCTAssertTrue(
            script.received.contains { $0.contains(Ops.loginCancel) },
            "取消应发 login.cancel 作废续接槽",
        )
    }

    func testCancelFromLimitScreenClearsCredentialsAndSendsCancelRPC() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        script.rule(Ops.loginVerify, #"{"status":"limit","enroll_token":"enroll-contract-fixed","devices":[]}"#)
        let (login, _) = makeModel(fake)

        await fillAndVerify(login)
        guard case .limit = login.phase else { return XCTFail("应落 Limit 相位") }
        login.cancel()
        XCTAssertEqual(login.phase, .form)
        XCTAssertNil(login.enrollToken, "踢选凭据清场")
        XCTAssertEqual(login.evictDevices, [])
        XCTAssertTrue(
            script.received.contains { $0.contains(Ops.loginCancel) },
            "取消应发 login.cancel 作废续接槽",
        )
    }

    func testVerifyLocalValidationAndErrorKeepFormPhase() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        let (login, _) = makeModel(fake)

        // 空验证码本地校验(不触 RPC)
        login.email = "kk@test.com"
        await login.verify()
        XCTAssertEqual(login.errorText, "请填写验证码")
        XCTAssertTrue(script.received.isEmpty, "本地校验失败不触 RPC")
        XCTAssertEqual(login.phase, .form)
    }

    // MARK: ok 收口记账(任意步进路径)

    func testOkFromEvictRecordsAccountIdentity() async throws {
        let fake = FakeDaemon()
        let script = StepScript()
        fake.onLine = StepScript.handler(script)
        script.rule(Ops.loginVerify, #"{"status":"limit","enroll_token":"enroll-contract-fixed","devices":[{"device_id":"dev-peer-fixed","name":"契约对端","online":false,"platform":"linux"}]}"#)
        script.rule(Ops.loginEvict, #"{"status":"ok","device_id":"dev-ios","name":"契约本机"}"#)
        let (login, account) = makeModel(fake)

        await fillAndVerify(login)
        await login.evict(deviceId: "dev-peer-fixed")
        XCTAssertEqual(login.phase, .done)
        // 账本记账随收口走(email = verify 提交值;实名以 ok 投影为准)
        XCTAssertEqual(account.email, "kk@test.com")
        XCTAssertEqual(account.deviceName, "契约本机")
    }
}
