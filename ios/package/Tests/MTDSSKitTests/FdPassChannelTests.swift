import XCTest
@testable import MTDSSKit

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
        XCTAssertEqual(Array(len), [0, 0, 0, UInt8(payload.count)], "payload 长度大端")
        XCTAssertEqual(frame.dropFirst(5), payload)
        XCTAssertEqual(frame.count, 5 + payload.count)
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
        // 短根(macOS 测试机 NSTemporaryDirectory ≈110B 会触发 FdPassChannel
        // 的 sun_path 守卫——生产 {home}/fds.sock 真机 ≈99B 可容;测试用
        // /tmp 短路径直构,socket 目录手建手清)。
        let dir = "/tmp/mtdss-t8-fdpass-\(Int.random(in: 100_000...999_999))"
        try? FileManager.default.removeItem(atPath: dir)
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

    // MARK: fd 随帧(V4-1 T10 SCM_RIGHTS,0x0D seek 源)

    /// sendmsg + SCM_RIGHTS 回环:客户端 0x0D 交付帧({request_id} 字母序)
    /// 携 1 个只读 fd,服务端 recvmsg 收帧收 fd → 存证副本读回字节一致 →
    /// 回 0x81 ack(daemon 口径)。拒开形(无 fd)沿 plain write 路径回归。
    func testChannelDeliversFileDescriptorOverUnixSocket() async throws {
        let dir = "/tmp/mtdss-t10-fdpass-\(Int.random(in: 100_000...999_999))"
        try? FileManager.default.removeItem(atPath: dir)
        try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        let sockPath = dir + "/fds.sock"
        let server = try UnixFdServer(path: sockPath)
        defer { server.close() }

        let content = Data("FD-PASS-CONTENT-42".utf8)
        let fileURL = URL(fileURLWithPath: dir + "/asset.bin")
        try content.write(to: fileURL)
        let handle = try FileHandle(forReadingFrom: fileURL)
        defer { try? handle.close() }

        let channel = FdPassChannel(socketPath: sockPath)
        let ack = try await channel.send(
            kind: FdPassKind.seekFD,
            payload: Data("{\"request_id\":5}".utf8),
            fileDescriptor: handle.fileDescriptor,
        )

        XCTAssertEqual(ack.kind, FdPassKind.ack, "daemon 口径:0x0D 应答须回 ack")
        let seen = try server.waitForFrame()
        XCTAssertEqual(seen.kind, FdPassKind.seekFD)
        let sent = try JSONSerialization.jsonObject(with: seen.payload) as? [String: Any]
        XCTAssertEqual(sent?["request_id"] as? Int, 5, "交付形 = {request_id}")
        let fd = try XCTUnwrap(server.receivedFd, "服务端经 SCM_RIGHTS 收到 1 fd")
        XCTAssertEqual(try Self.readAll(fd: fd), content, "fd 读回字节与源一致")
        Darwin.close(fd)
    }

    /// 从 fd 读到底(测试面)。
    static func readAll(fd: Int32) throws -> Data {
        var out = Data()
        var buf = [UInt8](repeating: 0, count: 4096)
        while true {
            let n = buf.withUnsafeMutableBytes { raw in
                read(fd, raw.baseAddress!, 4096)
            }
            guard n > 0 else {
                if n < 0 { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .ENODEV) }
                break
            }
            out.append(contentsOf: buf[0..<n])
        }
        return out
    }
}

/// 测试内 AF_UNIX 服务端(SCM_RIGHTS 面):accept 一连接,recvmsg 收一帧
/// (携 0/1 fd),按 daemon 口径回 0x81 ack。帧恒小(≤8KiB),单次 recvmsg
/// 整帧到齐(UDS 保序;分片到齐归 daemon 侧逐帧读,本面不覆盖)。
final class UnixFdServer {
    private var fd: Int32
    private let path: String
    private let lock = NSLock()
    private var receivedStorage: FdPassFrame?
    private var receivedFdStorage: Int32 = -1
    private var clientFD: Int32 = -1

    init(path: String) throws {
        self.path = path
        fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw POSIXError(.ENODEV) }
        try? FileManager.default.removeItem(atPath: path)
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

    /// 服务端已收 fd(recvmsg SCM_RIGHTS;-1 = 无)。dup 存证副本。
    var receivedFd: Int32? {
        lock.withLock { receivedFdStorage >= 0 ? receivedFdStorage : nil }
    }

    /// 阻塞收一帧(测试线程调用)。
    func waitForFrame(timeout: TimeInterval = 5) throws -> FdPassFrame {
        let deadline = Date().addingTimeInterval(timeout)
        while true {
            if let f = lock.withLock({ receivedStorage }) {
                return f
            }
            guard Date() < deadline else { throw POSIXError(.ETIMEDOUT) }
            usleep(10_000)
        }
    }

    private func acceptLoop() {
        var addr = sockaddr()
        var len = socklen_t(MemoryLayout<sockaddr>.size)
        let c = Darwin.accept(fd, &addr, &len)
        guard c >= 0 else { return }
        clientFD = c
        var buf = [UInt8](repeating: 0, count: 8 * 1024)
        let space = DarwinCmsg.space(MemoryLayout<Int32>.size)
        let control = UnsafeMutableRawPointer.allocate(
            byteCount: space, alignment: MemoryLayout<Int>.alignment)
        defer { control.deallocate() }
        control.initializeMemory(as: UInt8.self, repeating: 0, count: space)
        var gotFd: Int32 = -1
        var ctlLen: socklen_t = 0
        let n: Int = buf.withUnsafeMutableBytes { raw -> Int in
            var iov = iovec(
                iov_base: UnsafeMutableRawPointer(mutating: raw.baseAddress!),
                iov_len: raw.count)
            return withUnsafeMutablePointer(to: &iov) { iovp -> Int in
                var msg = msghdr()
                msg.msg_iov = iovp
                msg.msg_iovlen = 1
                msg.msg_control = control
                msg.msg_controllen = socklen_t(space)
                let r = recvmsg(c, &msg, 0)
                ctlLen = msg.msg_controllen
                return r
            }
        }
        guard n > 0 else { return }
        if ctlLen >= socklen_t(DarwinCmsg.headerAligned) {
            let hdr = control.assumingMemoryBound(to: cmsghdr.self)
            if hdr.pointee.cmsg_level == SOL_SOCKET, hdr.pointee.cmsg_type == SCM_RIGHTS {
                gotFd = Darwin.dup(DarwinCmsg.data(of: hdr).load(as: Int32.self))
            }
        }
        // 帧解析(头 5B + 恰长 payload;整帧已在 buf 前 n 字节)
        guard n >= 5 else { return }
        let kind = buf[0]
        let plen = Int(buf[1]) << 24 | Int(buf[2]) << 16 | Int(buf[3]) << 8 | Int(buf[4])
        guard n >= 5 + plen else { return }
        let frame = FdPassFrame(kind: kind, payload: Data(buf[5..<(5 + plen)]))
        let id = (try? JSONSerialization.jsonObject(with: frame.payload) as? [String: Any])
            .flatMap { $0["request_id"] as? NSNumber }?.uint64Value ?? 0
        lock.withLock {
            receivedStorage = frame
            receivedFdStorage = gotFd
        }
        if let payload = try? JSONSerialization.data(
            withJSONObject: ["ok": true, "request_id": id],
            options: [.sortedKeys],
        ) {
            let out = try? FdPassFrame.encode(kind: FdPassKind.ack, payload: payload)
            _ = out?.withUnsafeBytes { raw in
                write(c, raw.bindMemory(to: UInt8.self).baseAddress, raw.count)
            }
        }
    }

    func close() {
        if clientFD >= 0 { Darwin.close(clientFD); clientFD = -1 }
        if fd >= 0 { Darwin.close(fd); fd = -1 }
        try? FileManager.default.removeItem(atPath: path)
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
        try? FileManager.default.removeItem(atPath: path)
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
        guard c >= 0 else { return }
        clientFD = c
        // daemon 口径:收一帧 → 立即回 0x81 ack(ok:true + 回显 request_id)。
        if let frame = try? readFrame(from: c, timeout: 5) {
            lock.withLock { receivedStorage = frame }
            let id = (try? JSONSerialization.jsonObject(with: frame.payload) as? [String: Any])
                .flatMap { $0["request_id"] as? NSNumber }?.uint64Value ?? 0
            if let payload = try? JSONSerialization.data(
                withJSONObject: ["ok": true, "request_id": id],
                options: [.sortedKeys],
            ) {
                let out = try? FdPassFrame.encode(kind: FdPassKind.ack, payload: payload)
                _ = out?.withUnsafeBytes { raw in
                    write(c, raw.bindMemory(to: UInt8.self).baseAddress, raw.count)
                }
            }
        }
    }

    private let lock = NSLock()
    private var receivedStorage: FdPassFrame?

    /// 服务端已收帧(accept 线程读入;未到 → nil)。
    var receivedFrame: FdPassFrame? { lock.withLock { receivedStorage } }

    /// 阻塞收一帧(测试线程调用;验收已收帧用)。
    func waitForFrame(timeout: TimeInterval = 5) throws -> FdPassFrame {
        let deadline = Date().addingTimeInterval(timeout)
        while true {
            if let f = lock.withLock({ receivedStorage }) {
                return f
            }
            guard Date() < deadline else { throw POSIXError(.ETIMEDOUT) }
            usleep(10_000)
        }
    }

    /// 阻塞从 fd 读一帧(帧头 5B + 恰长 payload)。
    private func readFrame(from fd: Int32, timeout: TimeInterval) throws -> FdPassFrame {
        let deadline = Date().addingTimeInterval(timeout)
        func readSome(_ buf: UnsafeMutableRawBufferPointer, offset: Int, want: Int) -> Int {
            read(fd, buf.baseAddress!.advanced(by: offset), want)
        }
        var header = [UInt8](repeating: 0, count: 5)
        var have = 0
        while have < 5 {
            guard Date() < deadline else { throw POSIXError(.ETIMEDOUT) }
            let n = header.withUnsafeMutableBytes { readSome($0, offset: have, want: 5 - have) }
            if n > 0 { have += n } else { usleep(10_000) }
        }
        let len = Int(header[1]) << 24 | Int(header[2]) << 16 | Int(header[3]) << 8 | Int(header[4])
        var payload = [UInt8](repeating: 0, count: len)
        var got = 0
        while got < len {
            guard Date() < deadline else { throw POSIXError(.ETIMEDOUT) }
            let n = payload.withUnsafeMutableBytes { readSome($0, offset: got, want: len - got) }
            if n > 0 { got += n } else { usleep(10_000) }
        }
        return FdPassFrame(kind: header[0], payload: Data(payload))
    }

    func close() {
        if clientFD >= 0 { Darwin.close(clientFD); clientFD = -1 }
        if fd >= 0 { Darwin.close(fd); fd = -1 }
        try? FileManager.default.removeItem(atPath: path)
    }
}
