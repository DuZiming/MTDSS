import MTDSSKit
import XCTest

/// 登录/会话模型单测(T10 brief Step 1,先红后绿;伪 daemon 脚本):
/// - 登录成功态翻转:request_code(dev 回显预填)→ verify(name=生效名上帧)
///   → SessionModel 翻转 + AccountStore 记账持久化;
/// - 登出→会话清理事件链:login.logout RPC → event.login(false) 收敛清空,
///   幂等登出不炸,事件收敛语义单独钉死;
/// - 契约帧形状:server/email/ca/name 逐字段上帧断言(canonical 与 golden 同源)。
@MainActor
final class LoginFlowTests: XCTestCase {

    // MARK: 伪 daemon 脚本

    /// 登录链脚本:request_code 回显码;verify 回 device_id+名字透传并广播
    /// event.login(true);logout 回 {} 并广播 event.login(false);status 按
    /// logged_in 脚本应答。请求行留痕供帧形状断言(线程安全)。
    private final class LoginScript: @unchecked Sendable {
        private let lock = NSLock()
        private var lines: [String] = []
        var verifyError: String? = nil
        var statusLoggedIn = true

        func record(_ line: String) {
            lock.lock()
            defer { lock.unlock() }
            lines.append(line)
        }

        var received: [String] { lock.withLock { lines } }

        static func result(id: JSONValue, _ result: JSONValue) -> Data {
            Data((JSONValue.object(["id": id, "result": result]).canonical + "\n").utf8)
        }

        static func error(id: JSONValue, code: Int64, message: String) -> Data {
            Data((JSONValue.object(["id": id, "error": .object([
                "code": .number(String(code)),
                "message": .string(message),
            ])]).canonical + "\n").utf8)
        }

        static func event(_ method: String, _ params: JSONValue) -> Data {
            Data((JSONValue.object([
                "jsonrpc": .string("2.0"),
                "method": .string(method),
                "params": params,
            ]).canonical + "\n").utf8)
        }

        static func handler(_ script: LoginScript) -> FakeDaemon.LineHandler {
            { conn, line in
                let text = String(decoding: line, as: UTF8.self)
                script.record(text)
                guard let frame = try? JSONParser.parse(text),
                      let method = frame["method"].flatMap({ try? $0.asString() }),
                      case .string(let idRaw)? = frame["id"] else { return }
                let id = JSONValue.string(idRaw)
                switch method {
                case Ops.hello:
                    conn.sendLine(Self.result(id: id, .object([
                        "logged_in": .bool(script.statusLoggedIn),
                        "protocol": .number("1"),
                        "version": .string("t10-test"),
                    ])))
                case Ops.status:
                    conn.sendLine(Self.result(id: id, .object([
                        "logged_in": .bool(script.statusLoggedIn),
                        "online": .bool(true),
                        "recv_dir": .string("/tmp/inbox"),
                        "device": script.statusLoggedIn
                            ? .object(["device_id": .string("dev-ios"), "name": .null])
                            : .null,
                    ])))
                case Ops.loginRequestCode:
                    // dev 模式:验证码回显进应答(daemon 原样透传)
                    conn.sendLine(Self.result(id: id, .object(["code": .string("654321")])))
                case Ops.loginVerify:
                    guard let params = frame["params"],
                          params["email"] != nil,
                          let name = params["name"].flatMap({ try? $0.asString() }) else { return }
                    if let err = script.verifyError {
                        conn.sendLine(Self.error(id: id, code: 1, message: err))
                        return
                    }
                    conn.sendLine(Self.result(id: id, .object([
                        "device_id": .string("dev-ios"),
                        "name": .string(name),
                    ])))
                    // 翻转成功后广播 event.login(状态机同款:失败不发)
                    conn.sendLine(Self.event("event.login", .object(["logged_in": .bool(true)])))
                case Ops.loginLogout:
                    conn.sendLine(Self.result(id: id, .object([:])))
                    // 幂等登出同样广播 false(state.rs:407)
                    conn.sendLine(Self.event("event.login", .object(["logged_in": .bool(false)])))
                default:
                    break
                }
            }
        }
    }

    // MARK: 夹具

    private func makeRepo(_ fake: FakeDaemon) -> MtdssRepository {
        MtdssRepository(client: IpcClient(endpoint: fake))
    }

    private func makeAccount() -> AccountStore {
        AccountStore(defaults: UserDefaults(suiteName: "t10-login-\(UUID().uuidString)")!)
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

    // MARK: 登录成功态翻转

    func testLoginSuccessFlipsSessionAndStoresIdentity() async throws {
        let fake = FakeDaemon()
        let script = LoginScript()
        fake.onLine = LoginScript.handler(script)
        let repo = makeRepo(fake)
        let account = makeAccount()
        let session = SessionModel()
        let login = LoginModel(repo: repo, account: account, defaultDeviceName: "iPhone")

        // 预填:账号账本的 email/高级选项进表单(server 缺省烤入)
        login.email = "kk@test.com"

        // ① 获取验证码:dev 回显直接预填(桌面/android 同款便利)
        await login.requestCode()
        XCTAssertEqual(login.devCode, "654321")
        XCTAssertEqual(login.code, "654321", "dev 回显码应预填")
        XCTAssertNil(login.errorText)

        // request_code 帧:server=烤入缺省 + email(ca 空则键省略)
        let reqLine = try XCTUnwrap(script.received.first { $0.contains(Ops.loginRequestCode) })
        XCTAssertTrue(reqLine.contains("\"server\":\"https://www.mtdss.top\""), reqLine)
        XCTAssertTrue(reqLine.contains("\"email\":\"kk@test.com\""), reqLine)
        XCTAssertFalse(reqLine.contains("\"ca\""), "ca 空 = 键省略")

        // ② verify:成功翻转 + 身份记账
        await login.verify()
        XCTAssertNil(login.errorText)
        let result = try XCTUnwrap(login.succeeded, "verify 应成功")
        XCTAssertEqual(result.deviceId, "dev-ios")

        // verify 帧:name = 生效名(账本空 → 注入缺省),canonical 键序 code/email/name
        let verifyLine = try XCTUnwrap(script.received.first { $0.contains(Ops.loginVerify) })
        XCTAssertTrue(
            verifyLine.contains("\"code\":\"654321\",\"email\":\"kk@test.com\",\"name\":\"iPhone\""),
            verifyLine,
        )

        // App 装配面(LoginView onSuccess → session.applyLogin):登录态翻转
        session.applyLogin(result)
        XCTAssertEqual(session.loggedIn, true)
        XCTAssertEqual(session.deviceId, "dev-ios")

        // 账本记账(email/name 持久化;高级选项烤入值同记)
        XCTAssertEqual(account.email, "kk@test.com")
        XCTAssertEqual(account.deviceName, "iPhone")

        // 跨 store 重建(UserDefaults 持久)
        let reopened = AccountStore(defaults: account.underlyingDefaults)
        XCTAssertEqual(reopened.email, "kk@test.com")
        XCTAssertEqual(reopened.deviceName, "iPhone")
    }

    func testRenameOverridesNameOnNextLogin() async throws {
        let fake = FakeDaemon()
        let script = LoginScript()
        fake.onLine = LoginScript.handler(script)
        let account = makeAccount()
        account.setDeviceNameOverride("我的 iPad")
        let login = LoginModel(
            repo: makeRepo(fake),
            account: account,
            defaultDeviceName: "iPhone",
        )

        login.email = "kk@test.com"
        await login.requestCode()
        await login.verify()

        // 设置页改名(无运行时契约面)→ 下次 login.verify 的 name= 上帧生效
        let verifyLine = try XCTUnwrap(script.received.first { $0.contains(Ops.loginVerify) })
        XCTAssertTrue(verifyLine.contains("\"name\":\"我的 iPad\""), verifyLine)
        XCTAssertEqual(account.deviceName, "我的 iPad", "verify 实名回写账本")
    }

    func testRequestCodeLocalValidationSkipsRPC() async throws {
        let fake = FakeDaemon()
        let script = LoginScript()
        fake.onLine = LoginScript.handler(script)
        let login = LoginModel(repo: makeRepo(fake), account: makeAccount(), defaultDeviceName: "iPhone")

        login.email = "  "
        await login.requestCode()

        XCTAssertNotNil(login.errorText)
        XCTAssertTrue(script.received.isEmpty, "本地校验失败不触 RPC")
        XCTAssertFalse(login.busy)
    }

    func testVerifyErrorSurfacesAndKeepsSessionUntouched() async throws {
        let fake = FakeDaemon()
        let script = LoginScript()
        script.verifyError = "验证码错误"
        fake.onLine = LoginScript.handler(script)
        let session = SessionModel()
        let login = LoginModel(
            repo: makeRepo(fake),
            account: makeAccount(),
            defaultDeviceName: "iPhone",
        ) { result in
            session.applyLogin(result)
        }

        login.email = "kk@test.com"
        await login.requestCode()
        await login.verify()

        XCTAssertNotNil(login.errorText)
        XCTAssertNil(login.succeeded)
        XCTAssertNil(session.loggedIn, "失败不得翻转(事件不发的 daemon 侧契约 + onSuccess 未触发)")
        XCTAssertFalse(login.busy)
    }

    // MARK: 登出 → 会话清理事件链

    func testLogoutClearsSessionAndEventReasserts() async throws {
        let fake = FakeDaemon()
        let script = LoginScript()
        fake.onLine = LoginScript.handler(script)
        let repo = makeRepo(fake)
        let session = SessionModel()

        // 种子:已登录态(status 拉取)
        await session.refresh(repo: repo)
        XCTAssertEqual(session.loggedIn, true)
        XCTAssertEqual(session.deviceId, "dev-ios")

        // 事件流先于动作建立(Kotlin 订阅落位闸语义)
        let collector = EventCollector(repo.events)

        // 登出:RPC 幂等成功 + 乐观清空;event.login(false) 随后收敛
        try await session.logout(repo: repo)
        XCTAssertTrue(script.received.contains { $0.contains(Ops.loginLogout) })
        XCTAssertEqual(session.loggedIn, false)
        XCTAssertNil(session.deviceId, "登出清理 device_id(会话态不留尸)")

        // 事件到达:false 重申清空(不复活);true 事件不串(本脚本不发)
        let events = await collector.waitCount(1)
        guard case .login(let loggedIn)? = events.first else {
            return XCTFail("应收到 event.login(false),实收 \(events)")
        }
        XCTAssertFalse(loggedIn)
        session.consume(events.first!)
        XCTAssertEqual(session.loggedIn, false)
        XCTAssertNil(session.deviceId)
    }

    func testLogoutIdempotentWhenLoggedOut() async throws {
        let fake = FakeDaemon()
        let script = LoginScript()
        fake.onLine = LoginScript.handler(script)
        let repo = makeRepo(fake)
        let session = SessionModel()

        // 未登录(refresh:logged_out 脚本)直接登出:daemon 幂等成功不炸
        script.statusLoggedIn = false
        await session.refresh(repo: repo)
        XCTAssertEqual(session.loggedIn, false)
        try await session.logout(repo: repo)
        XCTAssertEqual(session.loggedIn, false)
    }

    func testLoginEventFalseClearsResidualSession() {
        let session = SessionModel()
        // 迟到/对账形状:会话残留(如 verify 成功后断连重连)时 false 收敛清空
        session.applyLogin(LoginVerifyResult(deviceId: "dev-x", name: "旧名"))
        XCTAssertEqual(session.loggedIn, true)

        session.consume(.login(loggedIn: false))
        XCTAssertEqual(session.loggedIn, false)
        XCTAssertNil(session.deviceId)
    }

    func testOnlineAndLoginEventsUpdateProjection() {
        let session = SessionModel()
        session.consume(.online(online: true))
        XCTAssertEqual(session.online, true)
        session.consume(.login(loggedIn: true))
        XCTAssertEqual(session.loggedIn, true, "true 事件置位(全量刷新由 status 层补)")
    }
}
