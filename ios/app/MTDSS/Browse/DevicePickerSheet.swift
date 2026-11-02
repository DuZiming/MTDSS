/// 选设备弹层(spec §4.3,mockup flows「device-picker」像素参照):
/// 半屏 sheet(含系统抓手),标题「发送 N 项到…」+ 取消(左);设备行同
/// 设备页语言 = 平台图标 + 名称 + 「平台 · 在线/离线」+ 状态点(绿 #34c759 /
/// 灰)(R4 真机轮 #2:设备行不显 ›——箭头只属设备内目录浏览行);**在线可
/// 选、离线置灰不可选**;点设备即发起;下滑/遮罩关闭(系统 sheet 语义)。
/// 设备列表 = Repository.devices 现拉,滤除本机。

import MTDSSKit
import SwiftUI

struct DevicePickerSheet: View {
    /// 标题(「发送 N 项到…」;转发 T7 换动词)。
    let title: String
    let repo: MtdssRepository
    /// 本机 device id(滤除,不给发给自己)。
    let selfDeviceId: String?
    /// 额外滤除(T7 转发:来源设备 A 不可选,daemon 拒 from==to 前置拦截)。
    var excluding: String? = nil
    /// 点在线设备即回调(发起由调用方承担)。
    let onPick: (DirectoryDevice) -> Void

    @State private var devices: [DirectoryDevice] = []
    @State private var loading = true
    @State private var loadError: String?

    var body: some View {
        VStack(spacing: 0) {
            header
            if loading {
                Spacer()
                ProgressView()
                Spacer()
            } else if let loadError {
                Spacer()
                ContentUnavailableView {
                    Label("设备列表不可用", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(loadError)
                }
                Spacer()
            } else if onlineDevices.isEmpty && offlineDevices.isEmpty {
                Spacer()
                ContentUnavailableView(
                    "暂无其他设备",
                    systemImage: "display",
                    description: Text("其他设备登录同一账号后出现在这里"),
                )
                Spacer()
            } else {
                deviceList
            }
        }
        // R3 真机轮 #4:.medium(≈半屏)太矮——设备多时挤压且标题贴角;
        // 0.65 屏高给列表呼吸,又不至于 .large 的全屏压迫感。
        .presentationDetents([.fraction(0.65)])
        .presentationDragIndicator(.visible)
        .task { await load() }
    }

    // MARK: 头部(取消左 + 标题居中 + 抓手)

    private var header: some View {
        VStack(spacing: 0) {
            ZStack {
                Text(title)
                    .font(.system(size: 17, weight: .semibold))
                    .lineLimit(1)
                HStack {
                    Button("取消") { dismiss() }
                        .font(.system(size: 17))
                        .foregroundStyle(Color.accentColor)
                    Spacer()
                }
            }
            .padding(.horizontal, 20)
            // R3 #4:顶距从抓手/圆角让开(.medium 时代 8pt 标题几乎贴角)
            .padding(.top, 16)
            .padding(.bottom, 10)
        }
    }

    // MARK: 设备列表(在线在前)

    private var onlineDevices: [DirectoryDevice] { devices.filter(\.online) }
    private var offlineDevices: [DirectoryDevice] { devices.filter { !$0.online } }

    private var deviceList: some View {
        ScrollView {
            VStack(spacing: 0) {
                ForEach(onlineDevices + offlineDevices, id: \.deviceId) { device in
                    DevicePickerRow(device: device) {
                        guard device.online else { return }
                        dismiss()
                        onPick(device)
                    }
                }
            }
        }
    }

    // MARK: 数据

    private func load() async {
        loading = devices.isEmpty
        loadError = nil
        do {
            let result = try await RpcGuard.withTimeout(seconds: 10) { [repo] in
                try await repo.devices()
            }
            devices = result.devices.filter {
                $0.deviceId != selfDeviceId && $0.deviceId != excluding
            }
            loading = false
        } catch {
            loading = false
            loadError = BrowseViewModel.userErrorText(for: error)
        }
    }

    @Environment(\.dismiss) private var dismiss
}

/// 设备行(mockup .drow:高 60,内缩 20;离线整行 0.45 置灰禁点)。
/// R4 真机轮 #2:不显 ›——箭头只属设备内目录浏览行(设备行/弹层行一致,
/// 可点性由按压反馈与状态点承担)。设备页列表与弹层复用本组件。
/// R5b #3:本机行(isSelf)副文 = 「平台 · 本机」且不可进(浏览自己 =
/// 首页收件箱冗余;弹层恒滤己,isSelf 仅设备页传 true)。
struct DevicePickerRow: View {
    let device: DirectoryDevice
    /// 本机行(设备页传;弹层不传)——副文本机徽章 + 禁点。
    var isSelf: Bool = false
    var onPick: () -> Void

    var body: some View {
        Button(action: onPick) {
            FullBleedRow(height: 60) {
                HStack(spacing: 13) {
                    Image(systemName: DeviceIcon.symbol(device.platform))
                        .font(.system(size: 22))
                        .foregroundStyle(Color(uiColor: .systemGray))
                        .frame(width: 30)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(device.name ?? device.deviceId)
                            .font(.system(size: 17))
                            .foregroundStyle(Color.primary)
                            .lineLimit(1)
                        Text(DevicesRowFormat.subtitle(
                            platform: device.platform,
                            isSelf: isSelf,
                            online: device.online,
                        ))
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
                .opacity(device.online ? 1 : 0.45)
            }
            // 整行触区(R3 真机轮 #5:设备行同浏览行,透明区可点)
            .contentShape(Rectangle())
        }
        .buttonStyle(PressableRowStyle())
        .disabled(!device.online || isSelf)
        .accessibilityLabel(Text("\(device.name ?? device.deviceId),\(isSelf ? "本机" : device.online ? "在线" : "离线")"))
    }
}
