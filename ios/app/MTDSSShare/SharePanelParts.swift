/// 分享面板共享件(R6 #4a;单文件双目标编译——主 App 的 DEBUG 状态页与
/// 扩展面板同源渲染,防两处漂移。xcodegen:MTDSS target 额外源指入本文件):
/// - ``ShareDocIcon``:问号文档图标(mockup 项目行);
/// - ``ShareRowState``:设备行状态(idle 状态点 / busy spinner / launched
///   绿勾「已发送」——R6 #4a 点选即忙:3s open() 竞窗全程行内可见);
/// - ``ShareDeviceRow``:设备行(R11 ② 对齐 App 行语言:平台图标 + 名 +
///   平台·在线副文 + 状态点,**无 › 箭头**——R4 真机轮 #2 App 侧先案:
///   箭头只属设备内目录浏览行,设备行无进页箭头;按压反馈同主 App
///   PressableRowStyle 语言。离线/不支持/过期整行置灰禁点不变)。

import MTDSSKit
import SwiftUI

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

/// 设备行状态(R6 #4a):idle=状态点(在线绿/离线灰);busy=行内 spinner
/// (点选即忙);launched=绿勾(已发送,短暂停留后扩展收口)。
enum ShareRowState: Equatable {
    case idle
    case busy
    case launched
}

/// 行按压反馈(主 App PressableRowStyle 同语言:按下行变暗;.plain 无任何
/// 反馈)。独立命名——本文件在主 App target 双编译,与 Common/RowStyles
/// 同名会撞。
struct ShareRowPressStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label.opacity(configuration.isPressed ? 0.6 : 1)
    }
}

/// 设备行(R11 ② 对齐 App 设备行语言,主 App ``DevicePickerRow`` 同构:
/// 平台图标(22pt/30 列)+ 名 17 + 副文 12.5 灰 + 状态区;**无 › 箭头**
/// ——R4 真机轮 #2 App 侧先案:箭头只属设备内目录浏览行,设备行无进页
/// 箭头)。`enabled` = 可点(缓存新鲜/实时 + 在线 + 平台支持 + 无他行
/// busy);他行 busy 时整表禁点(din 置灰)。状态区(R6 #4a):busy
/// spinner / launched 绿勾 / idle 状态点(9pt,App 同径)。
struct ShareDeviceRow: View {
    let device: SharedDeviceEntry
    let sub: String
    let enabled: Bool
    /// 行状态(busy/launched 仅该行自持;他行 busy 时本行仍 idle 但禁点)
    var state: ShareRowState = .idle
    let onPick: () -> Void

    var body: some View {
        Button(action: onPick) {
            HStack(spacing: 13) {
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
                statusArea
            }
            .padding(.horizontal, 20)
            .frame(minHeight: 60)
            .opacity(enabled || state != .idle ? 1 : 0.45)
            .contentShape(Rectangle())
        }
        .buttonStyle(ShareRowPressStyle())
        .disabled(!enabled)
        .accessibilityLabel(Text("\(device.name ?? device.deviceId),\(sub)"))
    }

    /// 状态区(R6 #4a 三态):busy = spinner 换状态点;launched = 绿勾;
    /// idle = 在线绿点/离线灰点(9pt 裸点右贴 20pt 内缩——与主 App
    /// DevicePickerRow 状态点同位像素对齐;busy/launched 形大才占 18 列)。
    @ViewBuilder private var statusArea: some View {
        switch state {
        case .busy:
            ProgressView()
                .frame(width: 18, height: 18)
        case .launched:
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 18))
                .foregroundStyle(Color(red: 0.20, green: 0.78, blue: 0.35))
                .frame(width: 18, height: 18)
        case .idle:
            Circle()
                .fill(device.online ? Color(red: 0.20, green: 0.78, blue: 0.35) : Color(uiColor: .systemGray4))
                .frame(width: 9, height: 9)
        }
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

// MARK: 直发进度面(R10;点设备后的表内进度条——用户裁定:选设备 → 进度条
// → 完成。扩展面板与主 App DEBUG 状态页同源渲染)

/// 逐文件进度行(名 + 细进度条 + 状态:完成 ✓ / 在途百分比 / 待发)。
struct ShareSendRow: View {
    let file: DirectSendProgress.FileState

    var body: some View {
        HStack(spacing: 13) {
            statusIcon
            VStack(alignment: .leading, spacing: 4) {
                Text(file.name)
                    .font(.system(size: 16))
                    .lineLimit(1)
                ProgressView(value: fraction)
                    .progressViewStyle(.linear)
                    .tint(fraction >= 1 ? Color(red: 0.20, green: 0.78, blue: 0.35) : Color.accentColor)
                Text(statusText)
                    .font(.system(size: 12.5))
                    .foregroundStyle(Color(uiColor: .systemGray))
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 22)
        .padding(.vertical, 7)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(Text("\(file.name),\(statusText)"))
    }

    private var fraction: Double {
        file.size > 0 ? Double(file.bytesDone) / Double(file.size) : (file.isDone ? 1 : 0)
    }

    private var statusIcon: some View {
        Group {
            if file.isDone {
                Image(systemName: "checkmark.circle.fill")
                    .foregroundStyle(Color(red: 0.20, green: 0.78, blue: 0.35))
            } else if file.isSending {
                ProgressView()
            } else {
                Image(systemName: "clock")
                    .foregroundStyle(Color(uiColor: .systemGray3))
            }
        }
        .font(.system(size: 18))
        .frame(width: 26)
    }

    private var statusText: String {
        if file.isDone { return "已完成" }
        if file.isSending {
            let pct = file.size > 0 ? Int(file.bytesDone * 100 / file.size) : 0
            return "传输中 \(pct)% · \(ShareFormat.bytes(file.bytesDone)) / \(ShareFormat.bytes(file.size))"
        }
        return "待发送 · \(ShareFormat.bytes(file.size))"
    }
}

/// 总进度条 + 取消钮(sending 相位页脚)。
struct ShareOverallBar: View {
    let progress: DirectSendProgress
    let onCancel: () -> Void

    var body: some View {
        VStack(spacing: 10) {
            HStack {
                Text("正在发送 \(progress.filesDone)/\(progress.filesTotal) 项")
                    .font(.system(size: 14, weight: .medium))
                Spacer()
                Text("\(progress.overallPercent)%")
                    .font(.system(size: 14, weight: .semibold))
                    .monospacedDigit()
            }
            ProgressView(value: Double(progress.overallPercent) / 100)
                .progressViewStyle(.linear)
                .tint(Color.accentColor)
            Button(action: onCancel) {
                Text("取消")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(Color(uiColor: .systemRed))
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 12)
                    .background(Color(uiColor: .systemGray6), in: RoundedRectangle(cornerRadius: 12))
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 20)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(Text("正在发送,总进度 \(progress.overallPercent)%,共 \(progress.filesTotal) 项"))
    }
}
