/// 分享扩展确认页(T11,spec §4.7 方案 A 定稿;mockup flows ② 像素参照):
/// 抓手 + 图标(44 蓝圆角)+「发送到 MTDSS」+「N 个项目」+ 项目清单
/// (问号文档 + 名 + 尺寸)+ 说明行 + **缓存设备列表**(App Group 读主
/// App 缓存;>5min 过期/缺失 = 全置灰 + 提示打开 App;离线/不支持平台
/// 单行置灰)。
/// **点选流状态机**(R6 #4a,``SharePickFlow``;20261102.4 真机轮:点设备
/// 后数秒无可见反应——点选即忙:该行 spinner 换状态点 + 整表禁点,3s
/// open() 竞窗全程可见;outcome:launched = 绿勾「已发送」短暂停留后扩展
/// 收口 / queued =「已排队,打开 MTDSS 即自动发送」注记即刻呈现;均可换
/// 设备重选)。open 官方路径不通时 responder-chain 绕法(VC 注入,spec
/// §4.7 备选;见 ``ShareViewController`` 风险注)。行状态渲染 =
/// ``ShareDeviceRow``(共享件,主 App DEBUG 状态页同源)。
/// 全程 os_log(R6:真机轮排障;Console 按 subsystem top.mtdss.ios.share)。

import MTDSSKit
import SwiftUI
import os

/// 扩展面板日志(真机轮排障面;每步一行,Console.app 可按类过滤)。
private let shareLog = Logger(subsystem: "top.mtdss.ios.share", category: "panel")

struct SharePanelView: View {
    let writer: ShareQueueWriter
    /// 扩展上下文(inputItems 来源;VC viewDidLoad 注入)。
    let context: NSExtensionContext
    /// NSExtensionContext.open 拉起宿主尝试(VC 注入;真机 iOS 26 实际
    /// 行为 T13 验证,失败即降级注记)。
    let openHost: (URL) async -> Bool
    /// responder-chain 拉起绕法(VC 注入;open 不通时的 spec §4.7 备选,
    /// R6 #4b)。
    let openHostFallback: (URL) -> Bool
    let onDone: () -> Void

    @State private var items: [SharePayload] = []
    @State private var loadFailures = 0
    @State private var loading = true
    @State private var deviceRows: [SharedDeviceEntry] = []
    @State private var cacheStale = false
    @State private var cacheMissing = false
    /// 点选流状态机(R6 #4a;busy/spinner/queued/launched 全随相走)。
    @State private var flow = SharePickFlow()
    @State private var errorText: String?

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
            if case .queued = flow.phase {
                // 降级路径(spec 4.7 风险注):两条拉起路都不通 → 队列保留,
                // 提示用户手动打开;「好」收口扩展
                VStack(spacing: 10) {
                    Text("已排队,打开 MTDSS 即自动发送")
                        .font(.system(size: 14, weight: .medium))
                    Button {
                        shareLog.info("queued note 好 → finish")
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

    // MARK: 设备行(在线可选;离线/不支持/缓存过期整行置灰;R6 #4a 状态区)

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
                        enabled: deviceEnabled(device) && listPickable,
                        state: rowState(device),
                        onPick: { enqueueAndOpen(device: device) },
                    )
                }
            }
        }
    }

    /// 整表可点(busy/launched 中禁点——点选即忙,R6 #4a;queued/failed
    /// 可换设备重选)。
    private var listPickable: Bool {
        switch flow.phase {
        case .idle, .queued, .failed: return true
        case .busy, .launched: return false
        }
    }

    /// 行状态(仅被点选行进 busy/launched;他行恒 idle 灰禁)。
    private func rowState(_ device: SharedDeviceEntry) -> ShareRowState {
        switch flow.phase {
        case .busy(let id) where id == device.deviceId: return .busy
        case .launched(let id) where id == device.deviceId: return .launched
        default: return .idle
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
        if case .launched(let id) = flow.phase, id == device.deviceId {
            return "\(platform) · 已发送"
        }
        return "\(platform) · 在线"
    }

    // MARK: 动作(点设备 = 入队 + 拉起主 App;R6 #4a 状态机驱动)

    private func enqueueAndOpen(device: SharedDeviceEntry) {
        guard loading == false, items.isEmpty == false else { return }
        guard flow.pick(deviceId: device.deviceId) else { return } // busy/launched 拒点
        errorText = nil
        shareLog.info("pick device=\(device.deviceId) name=\(device.name ?? "-", privacy: .public)")
        // MainActor 任务(评审 I-2:@State 变更与 ctx.open 发起同口径主线程;
        // 异步面仅剩 Kit 取消感知等待)
        Task { @MainActor in
            do {
                _ = try writer.enqueue(items: items, targetDeviceId: device.deviceId)
            } catch {
                let message = (error as? LocalizedError)?.errorDescription ?? "入队失败"
                shareLog.error("enqueue FAIL: \(message, privacy: .public)")
                flow.enqueueFailed(message: message)
                errorText = message
                return
            }
            shareLog.info("enqueue ok items=\(self.items.count) target=\(device.deviceId)")
            // 拉起主 App 两级(spec §4.7 + R6 #4b):
            // 1) ctx.open 官方路径:iOS 26 真机实证 completionHandler 部分
            //    形态永不回调——3s 竞窗(Kit firstOrTimeout,败方经取消感知
            //    续体真正收口;迟到回话被恰一次守卫吞掉)
            // 2) responder-chain 绕法(VC 注入;open 回话 false/超时才试,
            //    审核风险注见 ShareViewController)
            let opened = await firstOrTimeout(seconds: 3) {
                await openHost(AppGroup.openHostURL)
            } ?? false
            shareLog.info("ctx.open -> \(opened)")
            var fallbackOpened = false
            if !opened {
                fallbackOpened = openHostFallback(AppGroup.openHostURL)
                shareLog.info("responder-chain -> \(fallbackOpened)")
            }
            flow.openSettled(opened: opened, fallbackOpened: fallbackOpened)
            switch flow.phase {
            case .launched:
                // 拉起成功:短暂「已发送」绿勾停留(行内反馈),随即收口扩展
                // (系统切 App 面前 completeRequest;入队已落盘为准)
                shareLog.info("launched → finish in 0.6s")
                try? await Task.sleep(nanoseconds: 600_000_000)
                onDone()
            case .queued:
                // 降级注记即刻呈现(R6 #4a:queued 相位一行内出注记)
                shareLog.info("queued note shown")
            default:
                break
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
        shareLog.info("panel loaded items=\(loaded.count) failures=\(failures) devices=\(deviceRows.count) stale=\(cacheStale) missing=\(cacheMissing)")
    }
}
