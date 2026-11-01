/// 分享扩展确认页(T11,spec §4.7 方案 A 定稿;mockup flows ② 像素参照):
/// 抓手 + 图标(44 蓝圆角)+「发送到 MTDSS」+「N 个项目」+ 项目清单
/// (问号文档 + 名 + 尺寸)+ 说明行 + **缓存设备列表**(App Group 读主
/// App 缓存;>5min 过期/缺失 = 全置灰 + 提示打开 App;离线/不支持平台
/// 单行置灰)。点设备 = 两步拷入队列 + open() 尝试拉起主 App + 完成;
/// open() 失败降级「已排队,打开 MTDSS 即自动发送」注记(用户手动打开,
/// 队列消费兜底在主 App 前台/登录触发)。

import MTDSSKit
import SwiftUI

struct SharePanelView: View {
    let writer: ShareQueueWriter
    /// 扩展上下文(inputItems 来源;VC viewDidLoad 注入)。
    let context: NSExtensionContext
    /// NSExtensionContext.open 拉起宿主尝试(VC 注入;真机 iOS 26 实际
    /// 行为 T13 验证,失败即降级注记)。
    let openHost: (URL) async -> Bool
    let onDone: () -> Void

    @State private var items: [SharePayload] = []
    @State private var loadFailures = 0
    @State private var loading = true
    @State private var deviceRows: [SharedDeviceEntry] = []
    @State private var cacheStale = false
    @State private var cacheMissing = false
    @State private var busyTarget: String?
    @State private var errorText: String?
    /// open() 未通的降级态:「已排队,打开 MTDSS 即自动发送」。
    @State private var queuedNote = false

    var body: some View {
        VStack(spacing: 0) {
            grabber
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    header
                    itemSection
                    deviceHint
                    deviceSection
                    footerSpace
                }
            }
        }
        .background(Color(uiColor: .systemBackground))
        .task { await load() }
    }

    private var footerSpace: some View {
        VStack(spacing: 14) {
            if queuedNote {
                // 降级路径(spec 4.7 风险注):open 不通 → 队列保留,提示
                // 用户手动打开;「好」收口扩展
                VStack(spacing: 10) {
                    Text("已排队,打开 MTDSS 即自动发送")
                        .font(.system(size: 14, weight: .medium))
                    Button {
                        onDone()
                    } label: {
                        Text("好")
                            .font(.system(size: 16, weight: .semibold))
                            .foregroundStyle(.white)
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 12)
                            .background(Color.accentColor, in: RoundedRectangle(cornerRadius: 12))
                    }
                }
                .padding(.horizontal, 20)
            }
            if let errorText {
                Text(errorText)
                    .font(.system(size: 12.5))
                    .foregroundStyle(Color(uiColor: .systemRed))
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 20)
            }
            Spacer(minLength: 24)
        }
        .padding(.top, 10)
    }

    // MARK: 抓手 + 头部(图标 + 标题 + 项目数)

    private var grabber: some View {
        Capsule()
            .fill(Color(uiColor: .systemGray4))
            .frame(width: 36, height: 5)
            .frame(maxWidth: .infinity)
            .padding(.top, 8)
            .padding(.bottom, 4)
    }

    private var header: some View {
        HStack(spacing: 13) {
            RoundedRectangle(cornerRadius: 11)
                .fill(Color.accentColor)
                .frame(width: 44, height: 44)
                .overlay {
                    Image(systemName: "tray.and.arrow.up.fill")
                        .font(.system(size: 21))
                        .foregroundStyle(.white)
                }
            VStack(alignment: .leading, spacing: 2) {
                Text("发送到 MTDSS")
                    .font(.system(size: 18, weight: .semibold))
                Text(loading ? "正在读取…" : "\(items.count) 个项目")
                    .font(.system(size: 12.5))
                    .foregroundStyle(Color(uiColor: .systemGray))
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 22)
        .padding(.top, 10)
        .padding(.bottom, 12)
    }

    // MARK: 项目清单(问号文档 + 名 + 尺寸)

    @ViewBuilder private var itemSection: some View {
        if loading {
            HStack {
                Spacer()
                ProgressView()
                Spacer()
            }
            .padding(.vertical, 24)
        } else {
            VStack(spacing: 0) {
                ForEach(items) { item in
                    HStack(spacing: 13) {
                        ShareDocIcon()
                        VStack(alignment: .leading, spacing: 2) {
                            Text(item.name)
                                .font(.system(size: 16))
                                .lineLimit(1)
                            Text(ShareFormat.bytes(item.size))
                                .font(.system(size: 12.5))
                                .foregroundStyle(Color(uiColor: .systemGray))
                        }
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, 22)
                    .padding(.vertical, 7)
                }
                if loadFailures > 0 {
                    Text("\(loadFailures) 项无法作为文件读取,已跳过")
                        .font(.system(size: 12))
                        .foregroundStyle(Color(uiColor: .systemOrange))
                        .padding(.horizontal, 22)
                        .padding(.top, 4)
                }
            }
        }
    }

    /// 设备说明行(mockup:「选择目标设备(点选即排队并打开 MTDSS 自动发送)」;
    /// 缓存过期/缺失时换提示语并转警示色)。
    @ViewBuilder private var deviceHint: some View {
        if !loading {
            Text(cacheHint ?? "选择目标设备(点选即排队并打开 MTDSS 自动发送)")
                .font(.system(size: 12.5))
                .foregroundStyle(cacheHint == nil ? Color(uiColor: .systemGray) : Color(uiColor: .systemOrange))
                .padding(.horizontal, 20)
                .padding(.top, 10)
                .padding(.bottom, 6)
        }
    }

    private var cacheHint: String? {
        if cacheMissing { return "暂无设备缓存——先打开 MTDSS App 登录,再回到这里分享" }
        if cacheStale { return "设备列表已过期(打开 MTDSS App 可刷新)" }
        return nil
    }

    // MARK: 设备行(在线可选;离线/不支持/缓存过期整行置灰)

    @ViewBuilder private var deviceSection: some View {
        if !loading {
            VStack(spacing: 0) {
                if deviceRows.isEmpty {
                    Text(cacheMissing ? "先打开 MTDSS App 同步设备" : "账号下暂无其他设备")
                        .font(.system(size: 14))
                        .foregroundStyle(Color(uiColor: .systemGray))
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 18)
                }
                ForEach(deviceRows, id: \.deviceId) { device in
                    ShareDeviceRow(
                        device: device,
                        sub: deviceSub(device),
                        enabled: deviceEnabled(device) && busyTarget == nil,
                        onPick: { enqueueAndOpen(device: device) },
                    )
                }
            }
        }
    }

    /// 可选 = 缓存不过期 + 在线 + 平台支持(Ruling I:unix 桌面对端拒发,
    /// 扩展侧同口径置灰)。
    private func deviceEnabled(_ device: SharedDeviceEntry) -> Bool {
        guard !cacheStale, !cacheMissing, device.online else { return false }
        return SendService.peerInboxTarget(platform: device.platform) == .sentinel
    }

    private func deviceSub(_ device: SharedDeviceEntry) -> String {
        let platform = ShareDeviceRow.platformName(device.platform)
        if cacheStale || cacheMissing { return "\(platform) · 不可选" }
        guard device.online else { return "\(platform) · 离线" }
        if SendService.peerInboxTarget(platform: device.platform) != .sentinel {
            return "\(platform) · 暂不支持接收"
        }
        return "\(platform) · 在线"
    }

    // MARK: 动作(点设备 = 入队 + 拉起主 App)

    private func enqueueAndOpen(device: SharedDeviceEntry) {
        guard busyTarget == nil, loading == false, items.isEmpty == false else { return }
        busyTarget = device.deviceId
        errorText = nil
        // MainActor 任务(评审 I-2:@State 变更与 ctx.open 发起同口径主线程;
        // 异步面仅剩 Kit 取消感知等待)
        Task { @MainActor in
            do {
                _ = try writer.enqueue(items: items, targetDeviceId: device.deviceId)
            } catch {
                errorText = (error as? LocalizedError)?.errorDescription ?? "入队失败"
                busyTarget = nil
                return
            }
            // 拉起主 App:open() 官方受限 + completionHandler 部分形态永不
            // 回调——3s 竞窗(Kit firstOrTimeout,败方经取消感知续体真正
            // 收口);超时/失败 = 降级注记,队列保留
            let opened = await firstOrTimeout(seconds: 3) {
                await openHost(AppGroup.openHostURL)
            } ?? false
            if opened {
                onDone()
            } else {
                queuedNote = true
                busyTarget = nil
            }
        }
    }

    private func load() async {
        // 设备缓存先读(渲染即刻;文件装载随后,面板不空等)
        if let root = writer.store?.root {
            let cache = SharedDeviceCache(root: root)
            let now = Date()
            if let (devices, stale) = cache.readWithStaleness(now: now) {
                deviceRows = devices
                cacheStale = stale
                cacheMissing = false
            } else {
                deviceRows = []
                cacheStale = false
                cacheMissing = true
            }
        } else {
            deviceRows = []
            cacheMissing = true
        }
        let (loaded, failures) = await writer.loadItems(from: context)
        items = loaded
        loadFailures = failures
        loading = false
    }
}

/// 问号文档图标(mockup 项目行:doc 线框 + 小问号;与主 App FileTypeIcon 同象)。
struct ShareDocIcon: View {
    var body: some View {
        Image(systemName: "doc")
            .font(.system(size: 21))
            .foregroundStyle(Color(uiColor: .systemGray))
            .overlay {
                Text("?")
                    .font(.system(size: 9, weight: .bold))
                    .foregroundStyle(Color(uiColor: .systemGray))
                    .offset(y: 3)
            }
            .frame(width: 26)
    }
}

/// 设备行(mockup .drow:› + 平台图标 + 名 + 「平台 · 状态」 + 状态点;
/// 离线/不支持/过期整行 0.45 置灰禁点。平台图标/名 = 主 App DeviceIcon
/// 同语言的扩展侧复制——app 目标代码不进扩展,两处须同步改)。
struct ShareDeviceRow: View {
    let device: SharedDeviceEntry
    let sub: String
    let enabled: Bool
    let onPick: () -> Void

    var body: some View {
        Button(action: onPick) {
            HStack(spacing: 13) {
                Image(systemName: "chevron.right")
                    .font(.system(size: 15, weight: .medium))
                    .foregroundStyle(enabled ? Color.accentColor : Color(uiColor: .systemGray4))
                    .frame(width: 12)
                Image(systemName: Self.platformSymbol(device.platform))
                    .font(.system(size: 22))
                    .foregroundStyle(Color(uiColor: .systemGray))
                    .frame(width: 30)
                VStack(alignment: .leading, spacing: 2) {
                    Text(device.name ?? device.deviceId)
                        .font(.system(size: 17))
                        .foregroundStyle(Color.primary)
                        .lineLimit(1)
                    Text(sub)
                        .font(.system(size: 12.5))
                        .foregroundStyle(Color(uiColor: .systemGray))
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
                Circle()
                    .fill(device.online ? Color(red: 0.20, green: 0.78, blue: 0.35) : Color(uiColor: .systemGray4))
                    .frame(width: 9, height: 9)
            }
            .padding(.horizontal, 20)
            .frame(minHeight: 60)
            .opacity(enabled ? 1 : 0.45)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .accessibilityLabel(Text("\(device.name ?? device.deviceId),\(sub)"))
    }

    static func platformSymbol(_ platform: String?) -> String {
        switch platform?.lowercased() {
        case "windows", "linux", "macos": "display"
        case "ios": "iphone"
        case "android": "smartphone"
        default: "terminal"
        }
    }

    static func platformName(_ platform: String?) -> String {
        switch platform?.lowercased() {
        case "windows": "Windows"
        case "linux": "Linux"
        case "macos": "macOS"
        case "ios": "iOS"
        case "android": "Android"
        case .some(let other): other.capitalized
        case nil: "未知"
        }
    }
}
