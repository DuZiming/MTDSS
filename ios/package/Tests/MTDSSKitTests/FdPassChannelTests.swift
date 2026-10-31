import XCTest
import MTDSSKit

/// T8 fdpass 查询通道单测(crates/ipc/src/fdpass.rs 线格式的 Swift 客户端面;
/// 先红后绿):
/// - **编解码**:`kind u8 | len u32 BE | JSON` 逐字节口径(与 Kotlin/Rust
///   客户端共享线格式);超限 payload 拒编;截断/超限帧拒解;
/// - **回环**:真实 AF_UNIX 服务端(测试内 POSIX 监听)↔ FdPassChannel,
///   服务端按 daemon 口径回 ack 帧,客户端收齐。
final class FdPassChannelTests: XCTestCase {

    // MARK: 编解码

    func testEncodeFrameLayout() throws {
        let payload = Data("{\"ok\":true}".utf8)
        let frame = try FdPassFrame.encode(kind: FdPassKind.publishResult, payload: payload)
        // kind u8 + len u32 BE + payload
        XCTAssertEqual(frame.first, FdPassKind.publishResult)
        let len = frame.dropFirst().prefix(4)
        XCTAssertEqual(Array(len), [0, 0, 0, 9], "payload 长度大端")
        XCTAssertEqual(frame.dropFirst(5), payload)
        XCTAssertEqual(frame.count, 5 + 9)
    }

    func testDecodeRoundTrip() throws {
        let payload = Data("{\"exists\":false,\"request_id\":7,\"size\":0}".utf8)
        let bytes = try FdPassFrame.encode(kind: FdPassKind.destQuery, payload: payload)
        let frame = try FdPassFrame.decode(bytes)
        XCTAssertEqual(frame.kind, FdPassKind.destQuery)
        XCTAssertEqual(frame.payload, payload)
    }

    func testDecodeRejectsTruncatedAndOversized() {
        XCTAssertThrowsError(try FdPassFrame.decode(Data([0x04])), "截断帧头")
        // 声明长度超上限:拒解(防脏数据拖内存)
        var evil: [UInt8] = [0x04, 0xFF, 0xFF, 0xFF, 0xFF]
        evil.append(contentsOf: [0x7B] as [UInt8]) // '{'
        XCTAssertThrowsError(try FdPassFrame.decode(Data(evil)), "超限声明长度")
    }

    func testEncodeRejectsOversizedPayload() {
        let big = Data(count: FdPassFrame.maxPayloadBytes + 1)
        XCTAssertThrowsError(try FdPassFrame.checked(kind: 0x05, payload: big), "超限拒编")
    }

    // MARK: 回环(真实 AF_UNIX)

    func testChannelRoundTripAgainstUnixSocketServer() async throws {
        let dir = NSTemporaryDirectory() + "mtdss-t8-fdpass-\(UUID().uuidString)"
        try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        let sockPath = dir + "/fds.sock"
        let server = try UnixEchoServer(path: sockPath)
        defer { server.close() }

        let channel = FdPassChannel(socketPath: sockPath)
        let payload: [String: Any] = ["ok": true, "request_id": UInt64(3)]
        let ack = try await channel.send(kind: FdPassKind.publishResult, json: payload)

        XCTAssertEqual(ack.kind, FdPassKind.ack, "daemon 口径:0x05 应答须回 ack")
        let got = try JSONSerialization.jsonObject(with: ack.payload) as? [String: Any]
        XCTAssertEqual(got?["ok"] as? Bool, true)
        XCTAssertEqual(got?["request_id"] as? Int, 3)
        // 服务端收到的帧:kind 0x05 + JSON 逐字节可解
        let seen = try server.waitForFrame()
        XCTAssertEqual(seen.kind, FdPassKind.publishResult)
        let sent = try JSONSerialization.jsonObject(with: seen.payload) as? [String: Any]
        XCTAssertEqual(sent?["ok"] as? Bool, true)
        XCTAssertEqual(sent?["request_id"] as? Int, 3)
        server.close()
        try? FileManager.default.removeItem(atPath: dir)
    }

    func testSendToMissingSocketThrows() async throws {
        let channel = FdPassChannel(socketPath: "/nonexistent/mtdss-t8/fds.sock")
        do {
            _ = try await channel.send(kind: FdPassKind.destQuery, json: ["request_id": 1])
            XCTFail("不存在的 socket 必须抛错")
        } catch {
            // 预期路径:连接失败快速失败(不重试,daemon 同款口径)
        }
    }
}

/// 测试内 AF_UNIX 服务端:accept 一连接,收一帧,按 daemon 口径回 0x81 ack
/// (payload 回显 request_id + ok)。
final class UnixEchoServer {
    private var fd: Int32
    private let path: String
    private var clientFD: Int32 = -1

    init(path: String) throws {
        self.path = path
        fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw POSIXError(.ENODEV) }
        try FileManager.default.removeItem(atPath: path)
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        withUnsafeMutableBytes(of: &addr.sun_path) { buffer in
            bytes.withUnsafeBufferPointer { src in
                buffer.copyBytes(from: UnsafeRawBufferPointer(src).prefix(buffer.count - 1))
            }
        }
        let ok = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard ok == 0, Darwin.listen(fd, 1) == 0 else {
            Darwin.close(fd)
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .ENODEV)
        }
        Thread.detachNewThread { [weak self] in self?.acceptLoop() }
    }

    private func acceptLoop() {
        var addr = sockaddr()
        var len = socklen_t(MemoryLayout<sockaddr>.size)
        let c = Darwin.accept(fd, &addr, &len)
        if c >= 0 { clientFD = c }
    }

    /// 阻塞收一帧(测试线程调用)。
    func waitForFrame(timeout: TimeInterval = 5) throws -> FdPassFrame {
        let deadline = Date().addingTimeInterval(timeout)
        func readSome(_ buf: UnsafeMutableRawBufferPointer, offset: Int, want: Int) -> Int {
            read(clientFD, buf.baseAddress!.advanced(by: offset), want)
        }
        var header = [UInt8](repeating: 0, count: 5)
        var have = 0
        while have < 5 {
            guard Date() < deadline else { throw POSIXError(.ETIMEDOUT) }
            guard clientFD >= 0 else { throw POSIXError(.EBADF) }
            let n = header.withUnsafeMutableBytes { readSome($0, offset: have, want: 5 - have) }
            if n > 0 {
                have += n
            } else {
                usleep(10_000)
            }
        }
        let len = Int(header[1]) << 24 | Int(header[2]) << 16 | Int(header[3]) << 8 | Int(header[4])
        var payload = [UInt8](repeating: 0, count: len)
        var got = 0
        while got < len {
            guard Date() < deadline else { throw POSIXError(.ETIMEDOUT) }
            guard clientFD >= 0 else { throw POSIXError(.EBADF) }
            let n = payload.withUnsafeMutableBytes { readSome($0, offset: got, want: len - got) }
            if n > 0 {
                got += n
            } else {
                usleep(10_000)
            }
        }
        return FdPassFrame(kind: header[0], payload: Data(payload))
    }

    /// 回 ack 帧(daemon 口径)。
    func replyAck(ok: Bool, requestId: UInt64) throws {
        let payload = try JSONSerialization.data(
            withJSONObject: ["ok": ok, "request_id": requestId],
            options: [.sortedKeys],
        )
        let frame = try FdPassFrame.encode(kind: FdPassKind.ack, payload: payload)
        _ = frame.withUnsafeBytes { raw in
            write(clientFD, raw.baseAddress, raw.count)
        }
    }

    func close() {
        if clientFD >= 0 { Darwin.close(clientFD); clientFD = -1 }
        if fd >= 0 { Darwin.close(fd); fd = -1 }
        try? FileManager.default.removeItem(atPath: path)
    }
}
