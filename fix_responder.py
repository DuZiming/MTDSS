p = r"ios/package/Sources/MTDSSKit/Photo/ThumbResponder.swift"
data = open(p, "rb").read()
CRLF = b"\r\n"

def L(s):
    return s.encode("utf-8").replace(b"\n", CRLF)

# ═══ 修1:并行化(事件消费主环改并发 Task,钳 3) ═══

old_run = L(
    "    public func run(_ events: AsyncStream<IpcEvent>) async {\n"
    "        sweep()\n"
    "        for await event in events {\n"
    "            await handle(event)\n"
    "        }\n"
    "    }\n"
)
new_run = L(
    "    /// field r26f 修1:per-event 并发 Task(钳 3;事件互独立,request_id\n"
    "    /// 配对,乱序无害)。heavy 段(service.thumbData 内 Task.detached)\n"
    "    " ")已在 utility 线程;本侧 MainActor Task 在 await 点交错 = 真并发。\n"
    "    public func run(_ events: AsyncStream<IpcEvent>) async {\n"
    "        sweep()\n"
    "        let gate = ConcurrentGate(limit: 3)\n"
    "        for await event in events {\n"
    "            await gate.wait()\n"
    "            Task {\n"
    "                await handle(event)\n"
    "                gate.signal()\n"
    "            }\n"
    "        }\n"
    "    }\n"
)
assert old_run in data, "run loop"
data = data.replace(old_run, new_run, 1)

# ═══ 修2:O(1) 名→photoId 索引(替 O(n) 重放) ═══

# 索引类(插在 ThumbResponder 结构体前)
anchor_struct = L("/// 缩略图桥应答器:构造后随事件流 run(单消费)。")
assert anchor_struct in data
index_class = L(
    "/// 照片名→photoId 预建索引(field r26f 修2:替每请求 O(n) 重放——\n"
    "/// TTL 同步 assets 缓存 15s;0x06 清单同序 uniqueName 算法单点重放\n"
    "/// 的字典化,锁保护并发读)。\n"
    "final class PhotoNameIndex: @unchecked Sendable {\n"
    "    private let lock = NSLock()\n"
    "    private var cache: [String: (stamp: Date, map: [String: String])] = [:]\n"
    "    private let ttl: TimeInterval = 15\n"
    "\n"
    "    func map(for album: String, assets: [PhotoAsset]) -> [String: String] {\n"
    "        lock.lock()\n"
    "        if let hit = cache[album], Date().timeIntervalSince(hit.stamp) < ttl {\n"
    "            let m = hit.map\n"
    "            lock.unlock()\n"
    "            return m\n"
    "        }\n"
    "        lock.unlock()\n"
    "        var m: [String: String] = [:]\n"
    "        var used: Set<String> = []\n"
    "        for (offset, asset) in assets.enumerated() {\n"
    "            let raw = PhotoAssetNaming.displayName(\n"
    "                filename: asset.filename,\n"
    "                creationDateMs: asset.creationDateMs,\n"
    "                index: offset + 1)\n"
    "            let unique = PhotoAssetNaming.uniqueName(raw, used: &used)\n"
    "            m[unique] = asset.id\n"
    "        }\n"
    "        lock.lock()\n"
    "        cache[album] = (Date(), m)\n"
    "        lock.unlock()\n"
    "        return m\n"
    "    }\n"
    "}\n"
    "\n"
    "/// 并发闸(field r26f 修1:MainActor Task 钳;actor 信号量)。\n"
    "actor ConcurrentGate {\n"
    "    private var permits: Int\n"
    "    private var waiters: [CheckedContinuation<Void, Never>] = []\n"
    "\n"
    "    init(limit: Int) {\n"
    "        permits = limit\n"
    "    }\n"
    "\n"
    "    func wait() {\n"
    "        if permits > 0 {\n"
    "            permits -= 1\n"
    "        } else {\n"
    "            let cont = withCheckedContinuation { c in waiters.append(c) }\n"
    "            _ = cont\n"
    "        }\n"
    "    }\n"
    "\n"
    "    func signal() {\n"
    "        if waiters.isEmpty {\n"
    "            permits += 1\n"
    "        } else {\n"
    "            let cont = waiters.removeFirst()\n"
    "            cont.resume()\n"
    "        }\n"
    "    }\n"
    "}\n"
    "\n"
)
data = data.replace(anchor_struct, index_class + anchor_struct, 1)

# 结构体加 nameIndex 字段
old_init = L(
    "    /// 缩略图编码面(生产 = App 目标 IOSThumbEncoder)。\n"
    "    public let encoder: any ThumbEncoder\n"
)
new_init = L(
    "    /// 缩略图编码面(生产 = App 目标 IOSThumbEncoder)。\n"
    "    public let encoder: any ThumbEncoder\n"
    "    /// 名→photoId 索引(field r26f 修2;O(1) 查替 O(n) 重放)。\n"
    "    public let nameIndex = PhotoNameIndex()\n"
)
assert old_init in data, "struct fields"
data = data.replace(old_init, new_init, 1)

# 照片臂重写:字典查替重放
old_replay = L(
    "        do {\n"
    "            guard let entry = try await service.albums().first(where: { $0.name == album }) else {\n"
    "                return .failure(ThumbSourceError(reason: \"无此相册: \\(album)\"))\n"
    "            }\n"
    "            let assets = try await service.assets(albumId: entry.isRecents ? nil : entry.id)\n"
    "            // 批内唯一名重放(listEntries 同序同函数——0x06 面逐条唯一;\n"
    "            // 显示名 = 原始名(Android ThumbSource.displayName = asset.name\n"
    "            // 同面——重放比对用唯一名,生成件命名/分型凭据用原始名)\n"
    "            var used: Set<String> = []\n"
    "            for (offset, asset) in assets.enumerated() {\n"
    "                let raw = PhotoAssetNaming.displayName(\n"
    "                    filename: asset.filename, creationDateMs: asset.creationDateMs,\n"
    "                    index: offset + 1)\n"
    "                if PhotoAssetNaming.uniqueName(raw, used: &used) == name {\n"
    "                    let fd = try await service.openSeekable(photoId: asset.id)\n"
    "                    return .success(ThumbSource(fd: fd, displayName: raw, photoId: asset.id))\n"
    "                }\n"
    "            }\n"
    "            return .failure(ThumbSourceError(reason: \"源已不存在: \\(album)/\\(name)\"))\n"
    "        } catch let e as SeekOpenError {\n"
)
new_replay = L(
    "        do {\n"
    "            guard let entry = try await service.albums().first(where: { $0.name == album }) else {\n"
    "                return .failure(ThumbSourceError(reason: \"无此相册: \\(album)\"))\n"
    "            }\n"
    "            let assets = try await service.assets(albumId: entry.isRecents ? nil : entry.id)\n"
    "            // field r26f 修2:名→photoId O(1) 字典查(替 O(n) 重放;\n"
    "            // 索引随 TTL 15s 缓存,算法与 0x06 清单同序单点)。\n"
    "            let map = nameIndex.map(for: album, assets: assets)\n"
    "            guard let photoId = map[name] else {\n"
    "                return .failure(ThumbSourceError(reason: \"源已不存在: \\(album)/\\(name)\"))\n"
    "            }\n"
    "            // 显示名(生成件命名/分型凭据;原始名 = 去序号后缀)\n"
    "            let displayName = PhotoAssetNaming.stripIndexSuffix(name)\n"
    "            let fd = try await service.openSeekable(photoId: photoId)\n"
    "            return .success(ThumbSource(fd: fd, displayName: displayName, photoId: photoId))\n"
    "        } catch let e as SeekOpenError {\n"
)
assert old_replay in data, "replay"
data = data.replace(old_replay, new_replay, 1)

# ═══ 修3:0x0B 内联 JPEG(撤临时文件三跳) ═══

# deliver 重写:base64 直置 JSON
old_deliver = L(
    "    /// 编码产物落盘 → 0x0B `{ok, path, request_id}` → 0x81 ack 回执。**不删**\n"
    "    /// 生成件(引擎异步读——ack 只证明帧受理;引擎读毕 unlink,本侧挂载\n"
    "    /// 清扫兜底,serve-tmp/photo-serve 同哲学)。\n"
    "    func deliver(requestId: UInt64, displayName: String, bytes: Data) async {\n"
    "        try? FileManager.default.createDirectory(atPath: thumbDir, withIntermediateDirectories: true)\n"
    "        // 显示名单段化(资产/授权根名不含分隔符,防御性替换)\n"
    "        let dest = \"\\(thumbDir)/\\(requestId)-\\(displayName.replacingOccurrences(of: \"/\", with: \"_\")).jpg\"\n"
    "        do {"
)
new_deliver = L(
    "    /// field r26f 修3:0x0B 内联 JPEG(base64 直置 JSON;帧帽 8MiB 下\n"
    "    /// 缩略图恒适配。撤临时文件三跳(写→引擎读→删),I/O 面零文件)。\n"
    "    func deliver(requestId: UInt64, displayName: String, bytes: Data) async {\n"
    "        let b64 = bytes.base64EncodedString()\n"
    "        let payload: [String: Any] = [\n"
    "            \"ok\": true,\n"
    "            \"jpeg_b64\": b64,\n"
    "            \"request_id\": Int(clamping: requestId),\n"
    "        ]\n"
    "        _ = try? await fdpass.send(kind: FdPassKind.thumbData, json: payload)\n"
    "    }\n"
    "\n"
    "    /// [撤]field r26f 修3 撤除的旧路径(保留锚防止误回退):\n"
    "    /// 编码产物落盘 → 0x0B `{ok, path, request_id}` → 0x81 ack 回执。\n"
    "    @available(*, deprecated, message: \"field r26f 修3:走内联 JPEG\")\n"
    "    func deliverViaFile(requestId: UInt64, displayName: String, bytes: Data) async {\n"
    "        try? FileManager.default.createDirectory(atPath: thumbDir, withIntermediateDirectories: true)\n"
    "        // 显示名单段化(资产/授权根名不含分隔符,防御性替换)\n"
    "        let dest = \"\\(thumbDir)/\\(requestId)-\\(displayName.replacingOccurrences(of: \"/\", with: \"_\")).jpg\"\n"
    "        do {"
)
assert old_deliver in data, "deliver head"
data = data.replace(old_deliver, new_deliver, 1)
open(p, "wb").write(data)
print("ThumbResponder three fixes applied")
