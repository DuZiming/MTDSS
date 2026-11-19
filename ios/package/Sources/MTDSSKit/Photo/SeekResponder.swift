/// 可 seek fd 源桥应答器(V4-1 T10 预览波;daemon 侧 jobs.rs SeekBridge /
/// fdpass 0x0D 契约为真源,Android SeekResponder.kt 同构):消费 daemon 的
/// `fs.request_seek` 事件,开**可 seek 只读 fd** 经 fdpass **seek 通道**
/// (0x0D,SCM_RIGHTS 随帧)交付——预览按需读对手机源的原语面(视频随机
/// 读免整件导出);不可用 = 拒开(引擎落「导出再读」回退,轨 T3 已备)。
///
/// - **路径分流**(单 `path` = 完整 vpath,daemon 侧规范化):
///   ``PhotoVpath`` 前缀 → **照片臂**(相册/名 →
///   ``PhotoLibraryService/assets`` 批内唯一名重放(PhotoResponder 同序
///   同函数)解析 photoId → `openSeekable` 自有只读 fd);其余 → **文件臂**
///   (iOS 授权根 = 沙箱真实路径——收件箱 Documents/信任根,security
///   scope 由 TrustedRootsStore 持有,进程内引擎与应答器同沙箱,FileHandle
///   直开;即「授权根 SAF 等价物」的数据源面。iOS 无 SAF/bridge 根,引擎
///   对授权根直读 std::fs 不经本桥——本臂为同构防御面)。
/// - **权限门**(照片臂):仅 authorized/limited 应答(与 PhotoResponder
///   同门,非授权拒答);文件臂无门(沙箱即门)。
/// - **iCloud 未卸载**:`openSeekable` 抛 `.icloudNotLocal` → 拒开词面恒
///   `icloud_not_local`(稳定词;「原件在 iCloud,需先下载」提示语义,T25
///   经 ``PhotoLibraryService/openSeekable`` 直查消费;引擎侧照常落导出
///   回退——远端预览拉原件可达,自机预览由 T25 直查先行提示)。
/// - **拒开语言**:不可 seek 的探测**不在本侧**(MediaStore/SAF/资产 URL
///   直开文件恒可 seek;管道流等退化形由引擎落点 `from_seek_fd` 拒,同落
///   回退——Android 同口径)。根自身/目录面/相册缺席/名缺席/资产不可开
///   一律无 fd 0x0D + 词面。
/// - **生命周期**:App boot 时随 EventHub 订阅流 `run` 起来(事件流终结即
///   退,与引擎同寿;PhotoResponder 同款)。畸形事件帧在解析面丢弃(类型
///   化事件无缺字段形态);应答帧 ack ok:false = 请求已超时/未知,静默
///   收口(等待面已摘,事实以引擎侧读面为准)。

import Foundation

/// 照片图库虚拟根拆解(daemon photo.rs PHOTO_ROOT 同串;seek 源桥与后续
/// 缩略图桥 T11 **共用单点**防漂移——**纯词法切分**,Android photoVpathSub
/// 同构锚:根 → "",前缀剥除,尾 '/' 剥除)。逃逸形(`/照片图库/../x` 词面)
/// 不在此拒——落照片臂按「无此相册: ..」拒开(fail-closed;daemon 侧
/// photo_vpath 词法规范化先行,此形态本就路由不到 App)。
public enum PhotoVpath {
    /// 虚拟根 vpath。
    public static let root = "/照片图库"

    /// 完整 vpath → 根下子路径(相册/名);非本根形态 = nil。
    public static func sub(_ path: String) -> String? {
        if path == root { return "" }
        guard path.hasPrefix("\(root)/") else { return nil }
        return String(path.dropFirst(root.count + 1))
            .trimmingCharacters(in: CharacterSet(charactersIn: "/"))
    }
}

/// 文件臂 fd 面抽象(宿主单测注入;生产 = open(2) 直开)。
public protocol SeekOpeningFiles: Sendable {
    /// 路径 → **自有只读 fd**(调用方用毕即关);不可开抛错(词面透传拒开)。
    func open(path: String) throws -> Int32
}

/// 生产实现(open(2) 只读;iOS 授权根 = 沙箱真实路径,security scope 由
/// TrustedRootsStore 持有——进程内引擎与应答器同沙箱)。目录 fail-closed
/// (open(O_RDONLY) 对目录在 Darwin 上成功但读面 EISDIR——fstat 判目录即
/// 关 fd 抛错,Android「目录不可 seek(仅文件)」同口径)。
public struct DefaultSeekFiles: SeekOpeningFiles {
    public init() {}

    public func open(path: String) throws -> Int32 {
        let fd = Darwin.open(path, O_RDONLY)
        guard fd >= 0 else {
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .ENODEV)
        }
        var st = stat()
        if fstat(fd, &st) == 0 && (st.st_mode & S_IFMT) == S_IFDIR {
            Darwin.close(fd)
            throw POSIXError(.EISDIR)
        }
        return fd
    }
}

/// seek 源桥应答器:构造后随事件流 run(单消费)。
public struct SeekResponder: Sendable {

    /// 照片库数据源(App = PHPhotoLibraryService)。
    public let service: any PhotoLibraryService
    /// fdpass 查询通道(0x0D 应答,seek 通道单实例——daemon 侧 SeekBridge
    /// 单 request_id 计数器,与照片/服务桥空间各自独立)。
    public let fdpass: any FdPassFdAnswering
    /// 文件臂句柄面。
    public let files: any SeekOpeningFiles

    public init(
        service: any PhotoLibraryService,
        fdpass: any FdPassFdAnswering,
        files: any SeekOpeningFiles = DefaultSeekFiles(),
    ) {
        self.service = service
        self.fdpass = fdpass
        self.files = files
    }

    /// 事件流消费主环(fs.request_seek 应答,其余事件直通忽略——各屏订阅
    /// 面互不抢流)。
    public func run(_ events: AsyncStream<IpcEvent>) async {
        for await event in events {
            await handle(event)
        }
    }

    /// 单事件处置(测试直驱面)。
    public func handle(_ event: IpcEvent) async {
        guard case .requestSeek(let requestId, let path) = event else {
            return
        }
        await answerSeek(requestId: requestId, path: path)
    }

    /// vpath → 可 seek 只读 fd。照片臂/文件臂分流(阻塞面:PhotoKit 拉取
    /// 在 App 实现内部主线程隔离,await 挂起不占协作线程)。
    func answerSeek(requestId: UInt64, path: String) async {
        if let sub = PhotoVpath.sub(path) {
            await answerPhoto(requestId: requestId, sub: sub)
        } else {
            await answerFile(requestId: requestId, path: path)
        }
    }

    // MARK: 照片臂(0x06 清单同序唯一名重放)

    /// 照片 seek:`相册/名` → 相册照片批内唯一名重放(PhotoResponder 原始
    /// 名优先、撞名 ` (n)` 顺延,daemon 侧树名即此处名)解析 photoId →
    /// `openSeekable`。根("")/无文件段/相册缺席/名缺席/资产不可开 = 失败
    /// 词面。
    func answerPhoto(requestId: UInt64, sub: String) async {
        guard await isAuthorized() else {
            await refuse(requestId: requestId, reason: "照片权限未授予")
            return
        }
        if sub.isEmpty {
            await refuse(requestId: requestId, reason: "照片图库根不可 seek(仅文件)")
            return
        }
        // 相册/名 切分(Kotlin substringBefore/After 同口径)
        let album: String
        let name: String
        if let slash = sub.firstIndex(of: "/") {
            album = String(sub[sub.startIndex..<slash])
            name = String(sub[sub.index(after: slash)...])
        } else {
            album = sub
            name = ""
        }
        if name.isEmpty {
            await refuse(requestId: requestId, reason: "相册不可 seek(仅文件): \(album)")
            return
        }
        do {
            guard let entry = try await service.albums().first(where: { $0.name == album }) else {
                await refuse(requestId: requestId, reason: "无此相册: \(album)")
                return
            }
            let assets = try await service.assets(albumId: entry.isRecents ? nil : entry.id)
            // 批内唯一名重放(listEntries 同序同函数——0x06 面逐条唯一)
            var used: Set<String> = []
            for (offset, asset) in assets.enumerated() {
                let raw = PhotoAssetNaming.displayName(
                    filename: asset.filename, creationDateMs: asset.creationDateMs,
                    index: offset + 1)
                if PhotoAssetNaming.uniqueName(raw, used: &used) == name {
                    let fd = try await service.openSeekable(photoId: asset.id)
                    await deliver(requestId: requestId, fd: fd)
                    return
                }
            }
            await refuse(requestId: requestId, reason: "源已不存在: \(album)/\(name)")
        } catch let e as SeekOpenError {
            await refuse(requestId: requestId, reason: e.word)
        } catch {
            await refuse(requestId: requestId, reason: "\(error)")
        }
    }

    /// 授权可答(authorized/limited;limited = 子集内照常)。
    func isAuthorized() async -> Bool {
        let status = await service.permissionStatus()
        return status == .authorized || status == .limited
    }

    // MARK: 文件臂(授权根沙箱真实路径)

    /// 文件 seek:绝对路径 → open(2) 只读 fd(iOS 授权根 = Documents/信任
    /// 根,security scope 由 TrustedRootsStore 持有;缺席/目录/不可读 =
    /// 失败词面)。
    func answerFile(requestId: UInt64, path: String) async {
        do {
            let fd = try files.open(path: path)
            await deliver(requestId: requestId, fd: fd)
        } catch {
            await refuse(requestId: requestId, reason: "源不可开: \(error)")
        }
    }

    // MARK: 帧化

    /// 已开 fd 的应答:0x0D(+fd)→ 0x81 ack 回执;SCM_RIGHTS 在 sendmsg 时
    /// 已复制内核引用,发送返回即关本侧 fd(Android deliver 的 release
    /// 同款);ack ok:false(请求已超时/未知)与发送失败同静默收口——等待
    /// 面已摘(fd 由引擎就地 Drop),事实以引擎侧读面为准。
    func deliver(requestId: UInt64, fd: Int32) async {
        defer { Darwin.close(fd) }
        do {
            _ = try await fdpass.sendSeekFd(requestId: requestId, fileDescriptor: fd)
        } catch {
            // 发送失败:连接断/超窗;fd 已随 defer 关闭,引擎侧等待面超时
            // 兜底(超时即摘,迟到无 fd 无残留)。
        }
    }

    /// ok:false 拒答(引擎按「桥不可用」收口落导出回退;词面仅日志消费,
    /// iCloud 形 = 稳定词 icloud_not_local)。
    func refuse(requestId: UInt64, reason: String) async {
        _ = try? await fdpass.sendSeekRefuse(requestId: requestId, error: reason)
    }
}
