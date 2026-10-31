import XCTest
import MTDSSKit

/// 设备页视图模型单测(T7 brief Step 1,先红后绿):伪 daemon 脚本 devices
/// 应答,钉
/// - 现拉过滤本机(不给发给自己;DevicePickerSheet 同口径);
/// - 在线在前(离线置灰沉底,spec §4.4 行序同 picker 语言);
/// - stale 透传(daemon 现拉失败回退缓存 → UI 可注记「缓存」);
/// - 错误态(daemon error 帧 → errorText,清列表,T5 BrowseViewModel 同款);
/// - 刷新 = 重拉(根页「···」=刷新,spec §4.1/§4.4);
/// - 请求帧形状 = 契约 devices {}(空 params)。
@MainActor
final class DevicesViewModelTests: XCTestCase {

    // MARK: 脚本助手

    private nonisolated static func deviceJSON(_ d: DirectoryDevice) -> JSONValue {
        .object([
            "device_id": .string(d.deviceId),
            "name": d.name.map { .string($0) } ?? .null,
            "platform": d.platform.map { .string($0) } ?? .null,
            "online": .bool(d.online),
        ])
    }

    private nonisolated static func devicesResponse(
        idRaw: String, devices: [DirectoryDevice], stale: Bool,
    ) -> String {
        JSONValue.object([
            "id": .number(idRaw),
            "result": .object([
                "devices": .array(devices.map(deviceJSON)),
                "stale": .bool(stale),
            ]),
        ]).canonical
    }

    private nonisolated static func errorResponse(idRaw: String, code: Int64, message: String) -> String {
        JSONValue.object([
            "id": .number(idRaw),
            "error": .object([
                "code": .number(String(code)),
                "message": .string(message),
            ]),
        ]).canonical
    }

    /// devices 应答脚本(可计数;错误注入按次序)。
    private final class ScriptBox: @unchecked Sendable {
        private let lock = NSLock()
        private var count = 0
        var responses: [(devices: [DirectoryDevice], stale: Bool)] = []
        var errors: [String] = []

        func next() -> String? {
            lock.lock()
            defer { lock.unlock() }
            let i = count
            count += 1
            if i < errors.count { return errors[i] }
            if i < responses.count {
                return devicesResponse(
                    idRaw: "\(i + 1)",
                    devices: responses[i].devices,
                    stale: responses[i].stale,
                )
            }
            return nil
        }

        func capturedCount() -> Int {
            lock.lock()
            defer { lock.unlock() }
            return count
        }
    }

    private nonisolated static func responder(_ box: ScriptBox) -> FakeDaemon.LineHandler {
        { conn, line in
            guard let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self)),
                  case .number(let idRaw)? = frame["id"],
                  case .object(let params)? = frame["params"],
                  params.isEmpty,
                  let response = box.next() else { return }
            conn.sendLine(Data((response + "\n").utf8))
        }
    }

    private func makeVM(_ fake: FakeDaemon, selfDeviceId: String? = "dev-self") -> DevicesViewModel {
        DevicesViewModel(
            repo: MtdssRepository(client: IpcClient(endpoint: fake)),
            selfDeviceId: selfDeviceId,
        )
    }

    // MARK: 现拉/过滤/排序

    func testLoadFiltersSelfAndOrdersOnlineFirst() async throws {
        let fake = FakeDaemon()
        let box = ScriptBox()
        box.responses = [(
            devices: [
                DirectoryDevice(deviceId: "dev-self", name: "本机", platform: "ios", online: true),
                DirectoryDevice(deviceId: "dev-b", name: "离线机", platform: "linux", online: false),
                DirectoryDevice(deviceId: "dev-a", name: "台式机", platform: "windows", online: true),
                DirectoryDevice(deviceId: "dev-c", name: "另一台", platform: "macos", online: false),
            ],
            stale: false,
        )]
        fake.onLine = Self.responder(box)
        let vm = makeVM(fake)
        await vm.load()

        XCTAssertNil(vm.errorText)
        XCTAssertFalse(vm.loading)
        XCTAssertFalse(vm.stale)
        // 本机滤除;在线在前(离线沉底)
        XCTAssertEqual(vm.devices.map(\.deviceId), ["dev-a", "dev-b", "dev-c"])
        XCTAssertEqual(vm.devices.first?.online, true)
        XCTAssertEqual(vm.devices.last?.online, false)
    }

    func testLoadWithoutSelfIdKeepsAllDevices() async throws {
        let fake = FakeDaemon()
        let box = ScriptBox()
        box.responses = [(
            devices: [
                DirectoryDevice(deviceId: "dev-self", name: "本机", platform: "ios", online: true),
                DirectoryDevice(deviceId: "dev-a", name: "台式机", platform: "windows", online: true),
            ],
            stale: false,
        )]
        fake.onLine = Self.responder(box)
        let vm = makeVM(fake, selfDeviceId: nil)
        await vm.load()

        // selfDeviceId 未知(AppModel status 未就绪)→ 不过滤(不误删)
        XCTAssertEqual(vm.devices.map(\.deviceId), ["dev-self", "dev-a"])
    }

    // MARK: stale 透传

    func testStaleFlagSurfacesFromDaemonFallback() async throws {
        let fake = FakeDaemon()
        let box = ScriptBox()
        box.responses = [(
            devices: [DirectoryDevice(deviceId: "dev-a", name: "台式机", platform: "windows", online: false)],
            stale: true,
        )]
        fake.onLine = Self.responder(box)
        let vm = makeVM(fake)
        await vm.load()

        XCTAssertTrue(vm.stale, "daemon 现拉失败回退缓存 → stale=true 透传(UI 注记缓存)")
        XCTAssertEqual(vm.devices.count, 1)
    }

    // MARK: 错误态(T5 同款:errorText + 清列表)

    func testLoadErrorSetsErrorTextAndClearsList() async throws {
        let fake = FakeDaemon()
        let box = ScriptBox()
        box.errors = ["第一次失败"]
        box.responses = [(
            devices: [DirectoryDevice(deviceId: "dev-a", name: "台式机", platform: "windows", online: true)],
            stale: false,
        )]
        fake.onLine = Self.responder(box)
        let vm = makeVM(fake)
        await vm.load()
        XCTAssertNotNil(vm.errorText)
        XCTAssertTrue(vm.devices.isEmpty)

        // 刷新(重试)成功 → 错误态收口
        await vm.load()
        XCTAssertNil(vm.errorText)
        XCTAssertEqual(vm.devices.map(\.deviceId), ["dev-a"])
    }

    // MARK: 刷新 = 重拉(根页「···」=刷新)

    func testRefreshRepullsDevicesFrame() async throws {
        let fake = FakeDaemon()
        let box = ScriptBox()
        box.responses = [
            (devices: [DirectoryDevice(deviceId: "dev-a", name: "台式机", platform: "windows", online: true)], stale: false),
            (
                devices: [
                    DirectoryDevice(deviceId: "dev-a", name: "台式机", platform: "windows", online: true),
                    DirectoryDevice(deviceId: "dev-b", name: "新上线", platform: "linux", online: true),
                ],
                stale: false,
            ),
        ]
        fake.onLine = Self.responder(box)
        let vm = makeVM(fake)
        await vm.load()
        XCTAssertEqual(vm.devices.count, 1)

        await vm.load() // 「···」=刷新 → 现拉第二帧
        XCTAssertEqual(vm.devices.count, 2, "刷新后新设备在列")
        XCTAssertEqual(box.capturedCount(), 2, "devices 请求共两发")
    }

    // MARK: 请求帧形状 = 契约 devices {}

    func testRequestFrameIsEmptyDevicesCall() async throws {
        let fake = FakeDaemon()
        let box = ScriptBox()
        box.responses = [([DirectoryDevice(deviceId: "dev-a", name: "n", platform: nil, online: true)], false)]
        fake.onLine = Self.responder(box)
        let vm = makeVM(fake)
        await vm.load()

        let frames = fake.connections.first?.received ?? []
        let devicesFrames = frames.filter {
            guard let frame = try? JSONParser.parse(String(decoding: $0, as: UTF8.self)),
                  case .string(let method)? = frame["method"] else { return false }
            return method == "devices"
        }
        XCTAssertEqual(devicesFrames.count, 1)
        let frame = try XCTUnwrap(devicesFrames.first.flatMap { try? JSONParser.parse(String(decoding: $0, as: UTF8.self)) })
        guard case .object(let params)? = frame["params"] else {
            return XCTFail("devices params 缺失")
        }
        XCTAssertTrue(params.isEmpty, "devices 无参数(golden devices.req.json)")
    }
}
