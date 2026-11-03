/// 分享扩展确认页(R10 直发形态;用户裁定 20261103.2:点设备 → 表内进度
/// 条 → 完成——无排队、无拉起主 App、无「好」钮;mockup flows ② 像素参照):
/// 抓手 + 图标 +「发送到 MTDSS」+ 项目清单 + **设备列表**(在线可选;
/// 点选 = ``DirectSendEngine`` 直发:暂存→发送→**表内进度条**
/// [逐文件行 + 总百分比 + 取消]→ 完成即收口)。**设备列表 = 引擎实时**
/// (R11 ①,用户报告「和 App 内不同步」:面板装载即早开机 `devices`
/// 现拉 ``ShareDeviceList/live``;缓存 = 引擎不可开机时的兜底,陈旧置灰
/// 语义只属该路径)。**列表与 App 设备页完全统一**(R12 ①,用户报告
/// 「列表不一致/排序不对齐/排除本机;Linux 在线却被置灰」):行序 = 在线
/// 在前组内保序(``DeviceListOrder`` 单份,App 设备页同函数)、本机行滤除
/// (缓存路径 = 快照 `self_device_id` 载荷,live 路径 = 引擎 status 自身
/// id)、**全平台可选**(Ruling I 客户端拒发退役,T5b Ruling J 引擎哨兵
/// 统一,Linux/macOS 在线行不再置灰)。直发不可行面(未同步/超帽 50 件
/// 200MB/boot 失败)
/// 自动降级既有队列(「已排队,打开 MTDSS 即自动发送」注记,主 App 下次
/// 打开消费);受理/传输失败 = 重试[重选目标]/转队列双钮。两级拉起代码
/// 退役(open()/responder-chain,R6/R9 真机实证不拉起)。行/进度渲染 =
/// ``SharePanelParts`` 共享件(主 App DEBUG 状态页同源)。全程 os_log
/// (subsystem top.mtdss.ios.share)。

import MTDSSKit
import SwiftUI
import os

/// 扩展面板日志(真机轮排障面;每步一行,Console.app 可按类过滤)。
private let shareLog = Logger(subsystem: "top.mtdss.ios.share", category: "panel")

struct SharePanelView: View {
    let writer: ShareQueueWriter
    /// 扩展上下文(inputItems 来源;VC viewDidLoad 注入)。
    let context: NSExtensionContext
    /// 直发编排(R10;VC 注入,面板生命周期 = 引擎生命周期)。
    let engine: DirectSendEngine
    let onDone: () -> Void

    @State private var items: [SharePayload] = []
    @State private var loadFailures = 0
    @State private var loading = true
    /// 设备列表来源(R11 ① Kit 状态机:cache 先渲染,live 换入,失败兜底)。
    @State private var deviceList = ShareDeviceList()
    /// 直发流状态盒(R10;busy→sending→done/cancelled/failed/queued;
    /// @Observable 盒 = 编排引用直改 + 视图观察,结构体状态机的 SwiftUI 面)。
    @State private var sendState = ShareSendState()
    @State private var errorText: String?

    /// 直发流状态(状态盒投影;body 内读取即被观察跟踪)。
    private var flow: ShareSendFlow { sendState.flow }

    // MARK: 设备列表投影(R11 ① 状态机 → 视图面;R12 ① 排序/滤己收口在
    //        Kit 状态机 ``ShareDeviceList/rows``,与 App 设备页同源同序)

    /// 行集(cache/live 两档同投影:self 滤除 + 在线在前;missing = 空)。
    private var deviceRows: [SharedDeviceEntry] {
        deviceList.rows
    }

    /// 缓存陈旧置灰面(只属兜底路径;live 换入即消失)。
    private var cacheStale: Bool {
        if case .cache(_, true) = deviceList.source { return true }
        return false
    }

    /// 缓存缺失(缺失/损坏;「先打开 MTDSS App」提示面)。
    private var cacheMissing: Bool {
        if case .missing = deviceList.source { return true }
        return false
    }

    var body: some View {
        VStack(spacing: 0) {
            grabber
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    header
                    mainSection
                    if showDevices {
                        deviceHint
                        deviceSection
                    }
                    footerSpace
                }
            }
        }
        .background(Color(uiColor: .systemBackground))
        .task { await load() }
    }

    // MARK: 相位面(显示分档)

    /// 设备列表可见相(idle/failed[重选] + busy 点选即忙;sending 起表内
    /// 即进度页,设备列隐藏)。
    private var showDevices: Bool {
        switch flow.phase {
        case .idle, .busy, .failed: return !loading
        case .sending, .done, .cancelled, .queued: return false
        }
    }

    /// 在途设备名(busy/sending 相位的 deviceId → 设备行名;查无回 id)。
    private var activeDeviceName: String? {
        let id: String?
        switch flow.phase {
        case .busy(let busyId): id = busyId
        case .sending(let sendId, _): id = sendId
        default: id = nil
        }
        guard let id else { return nil }
        return deviceRows.first(where: { $0.deviceId == id }).map { $0.name ?? id }
    }

    /// 头部副题(相位文案)。
    private var headerSub: String {
        switch flow.phase {
        case .idle: return loading ? "正在读取…" : "\(items.count) 个项目"
        case .busy: return "正在准备 → \(activeDeviceName ?? "设备")…"
        case .sending: return "正在发送到 \(activeDeviceName ?? "设备")…"
        case .done: return "已发送"
        case .cancelled: return "已取消"
        case .queued: return "已排队"
        case .failed: return "发送失败"
        }
    }

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
                Text(headerSub)
                    .font(.system(size: 12.5))
                    .foregroundStyle(Color(uiColor: .systemGray))
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 22)
        .padding(.top, 10)
        .padding(.bottom, 12)
    }

    // MARK: 主体(装载 spinner / 项目清单 / 直发进度行)

    @ViewBuilder private var mainSection: some View {
        if loading {
            HStack {
                Spacer()
                ProgressView()
                Spacer()
            }
            .padding(.vertical, 24)
        } else if case .sending = flow.phase, let progress = flow.progress {
            // 直发进度行(共享件;逐文件条 + 状态)
            VStack(spacing: 0) {
                ForEach(progress.files, id: \.name) { file in
                    ShareSendRow(file: file)
                }
                if loadFailures > 0 {
                    Text("\(loadFailures) 项无法作为文件读取,已跳过")
                        .font(.system(size: 12))
                        .foregroundStyle(Color(uiColor: .systemOrange))
                        .padding(.horizontal, 22)
                        .padding(.top, 4)
                }
            }
        } else {
            // 项目清单(idle/busy/failed/终态;问号文档 + 名 + 尺寸)
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

    // MARK: 设备说明 + 设备行(idle/busy/failed)

    @ViewBuilder private var deviceHint: some View {
        Text(deviceHintText)
            .font(.system(size: 12.5))
            .foregroundStyle(deviceHintWarning ? Color(uiColor: .systemOrange) : Color(uiColor: .systemGray))
            .padding(.horizontal, 20)
            .padding(.top, 10)
            .padding(.bottom, 6)
    }

    private var deviceHintWarning: Bool {
        cacheHint != nil || (!engine.available && !deviceRows.isEmpty)
    }

    private var deviceHintText: String {
        if let cacheHint { return cacheHint }
        if case .failed(_, let message) = flow.phase {
            return "发送失败:\(message)——可换设备重发,或转队列由 App 稍后发送"
        }
        if !engineAvailable {
            return "当前不可直发(主 App 未同步登录态),点选将排队、打开 App 自动发送"
        }
        return "选择目标设备(点选即发送)"
    }

    @ViewBuilder private var deviceSection: some View {
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
                    onPick: { pickDevice(device) },
                )
            }
        }
    }

    private var cacheHint: String? {
        if cacheMissing { return "暂无设备缓存——先打开 MTDSS App 登录,再回到这里分享" }
        if cacheStale { return "设备列表已过期(打开 MTDSS App 可刷新)" }
        return nil
    }

    /// 整表可点(busy/sending 禁点;failed 可换设备重发)。
    private var listPickable: Bool {
        switch flow.phase {
        case .idle, .failed: return true
        case .busy, .sending, .done, .cancelled, .queued: return false
        }
    }

    private func rowState(_ device: SharedDeviceEntry) -> ShareRowState {
        switch flow.phase {
        case .busy(let id) where id == device.deviceId: return .busy
        case .done(let id) where id == device.deviceId: return .launched
        default: return .idle
        }
    }

    /// 可选 = 缓存不过期 + 在线(R12 ①:平台不再参与——Ruling I 客户端
    /// 拒发已随 T5b Ruling J 引擎哨兵统一退役,Linux/macOS 在线行不再置灰;
    /// 本机行已在 ``ShareDeviceList/rows`` 滤除,不给发给自己)。
    private func deviceEnabled(_ device: SharedDeviceEntry) -> Bool {
        !cacheStale && !cacheMissing && device.online
    }

    private func deviceSub(_ device: SharedDeviceEntry) -> String {
        let platform = ShareDeviceRow.platformName(device.platform)
        if cacheStale || cacheMissing { return "\(platform) · 不可选" }
        guard device.online else { return "\(platform) · 离线" }
        return "\(platform) · 在线"
    }

    // MARK: 页脚(sending 总进度 + 取消 / queued 注记 / failed 转队列)

    @ViewBuilder private var footerSpace: some View {
        VStack(spacing: 14) {
            switch flow.phase {
            case .sending:
                if let progress = flow.progress {
                    ShareOverallBar(progress: progress) {
                        shareLog.info("cancel tap")
                        Task { await engine.cancel() }
                    }
                }
            case .done:
                HStack(spacing: 8) {
                    Image(systemName: "checkmark.circle.fill")
                        .foregroundStyle(Color(red: 0.20, green: 0.78, blue: 0.35))
                    Text("已发送")
                        .font(.system(size: 15, weight: .semibold))
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 10)
            case .queued:
                Text("已排队,打开 MTDSS 即自动发送")
                    .font(.system(size: 14, weight: .medium))
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 10)
            case .failed:
                Button {
                    queueFallbackTapped()
                } label: {
                    Text("转入队列,打开 MTDSS 时自动发送")
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundStyle(.white)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 12)
                        .background(Color.accentColor, in: RoundedRectangle(cornerRadius: 12))
                }
                .buttonStyle(.plain)
            case .idle, .busy, .cancelled:
                EmptyView()
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

    // MARK: 动作

    private var engineAvailable: Bool { engine.available }

    private func pickDevice(_ device: SharedDeviceEntry) {
        guard loading == false, items.isEmpty == false else { return }
        guard sendState.flow.pick(deviceId: device.deviceId) else { return } // busy/sending 拒点
        errorText = nil
        shareLog.info("pick device=\(device.deviceId) name=\(device.name ?? "-", privacy: .public)")
        Task { @MainActor in
            let outcome = await engine.run(device: device, payloads: items, state: sendState)
            await settle(outcome: outcome)
        }
    }

    /// 终局收口(sent 1s / cancelled·queued 短停后 complete;failed 等用户)。
    private func settle(outcome: DirectSendEngine.Outcome) async {
        switch outcome {
        case .sent:
            shareLog.info("done → finish in 1s")
            try? await Task.sleep(nanoseconds: 1_000_000_000)
            finish()
        case .cancelled:
            shareLog.info("cancelled → finish")
            try? await Task.sleep(nanoseconds: 600_000_000)
            finish()
        case .queued:
            shareLog.info("queued note shown → finish in 1.5s")
            try? await Task.sleep(nanoseconds: 1_500_000_000)
            finish()
        case .failed:
            shareLog.info("failed → awaiting user (retry/queue)")
        }
    }

    private func queueFallbackTapped() {
        guard case .failed(let deviceId, _) = flow.phase else { return }
        shareLog.info("queue fallback tap device=\(deviceId)")
        let outcome = engine.queueFallback(deviceId: deviceId, payloads: items, state: sendState)
        if outcome == .queued {
            Task { @MainActor in
                try? await Task.sleep(nanoseconds: 1_500_000_000)
                finish()
            }
        }
    }

    /// 收口(先停引擎/清批,再 completeRequest——落盘状态为准)。
    private func finish() {
        engine.shutdown()
        writer.cleanup()
        shareLog.info("completeRequest")
        onDone()
    }

    // MARK: 装载

    private func load() async {
        // 设备缓存先读(渲染即刻;live 换入随后,面板不空等)。selfDeviceId
        // = 快照载荷(R12 ①:缓存路径滤本机的源;live 换入后以引擎 id 覆写)
        if let root = writer.store?.root {
            let cache = SharedDeviceCache(root: root)
            let now = Date()
            if let (devices, stale, selfId) = cache.readWithStaleness(now: now) {
                deviceList.loadCache(devices: devices, stale: stale, selfDeviceId: selfId)
            } else {
                deviceList.loadCache(devices: nil)
            }
        } else {
            deviceList.loadCache(devices: nil)
        }
        let (loaded, failures) = await writer.loadItems(from: context)
        items = loaded
        loadFailures = failures
        loading = false
        engine.purgeOrphanBatches()
        shareLog.info("panel loaded items=\(loaded.count) failures=\(failures) devices=\(deviceRows.count) source=\(String(describing: deviceList.source), privacy: .public) directSend=\(engine.available)")
        // 设备列表实时面(R11 ①):引擎早开机(点选即发)+ devices 现拉
        // 换入;失败缓存兜底(状态机置位,提示面不变)
        await loadLiveDevices()
    }

    /// 引擎早开机 + devices 现拉(R11 ①;失败静默缓存兜底——降级同语言,
    /// 未同步形 ``available``=false 根本不 boot,提示照旧「点选将排队」)。
    /// R12 ①:换入时携引擎 status 自身 id(live 路径滤本机的权威源)。
    private func loadLiveDevices() async {
        guard engine.available else {
            deviceList.markLiveUnavailable()
            shareLog.info("live devices skipped: direct-send unavailable")
            return
        }
        do {
            let (devices, selfId) = try await engine.liveDevices()
            deviceList.adoptLive(devices, selfDeviceId: selfId)
            shareLog.info("live devices loaded count=\(devices.count) self=\(selfId ?? "-", privacy: .public)")
        } catch {
            deviceList.markLiveUnavailable()
            shareLog.info("live devices unavailable: \(String(describing: error), privacy: .public)")
        }
    }
}
