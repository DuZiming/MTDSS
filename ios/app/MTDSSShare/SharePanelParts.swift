/// 分享面板共享件(R6 #4a;单文件双目标编译——主 App 的 DEBUG 状态页与
/// 扩展面板同源渲染,防两处漂移。xcodegen:MTDSS target 额外源指入本文件):
/// - ``ShareDocIcon``:问号文档图标(mockup 项目行);
/// - ``ShareRowState``:设备行状态(idle 状态点 / busy spinner / launched
///   绿勾「已发送」——R6 #4a 点选即忙:3s open() 竞窗全程行内可见);
/// - ``ShareDeviceRow``:设备行(mockup .drow:› + 平台图标 + 名 + 状态 +
///   状态区;离线/不支持/过期整行置灰禁点)。平台图标/名 = 主 App
///   DeviceIcon 同语言的扩展侧复制。

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

/// 设备行(mockup .drow;R6 #4a 状态区三态)。`enabled` = 可点(缓存新鲜
/// + 在线 + 平台支持 + 无他行 busy);他行 busy 时整表禁点(din 置灰)。
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
                statusArea
            }
            .padding(.horizontal, 20)
            .frame(minHeight: 60)
            .opacity(enabled || state != .idle ? 1 : 0.45)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .accessibilityLabel(Text("\(device.name ?? device.deviceId),\(sub)"))
    }

    /// 状态区(R6 #4a 三态):busy = spinner 换状态点;launched = 绿勾;
    /// idle = 在线绿点/离线灰点。
    @ViewBuilder private var statusArea: some View {
        switch state {
        case .busy:
            ProgressView()
                .frame(width: 18, height: 18)
        case .launched:
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 18))
                .foregroundStyle(Color(red: 0.20, green: 0.78, blue: 0.35))
                .frame(width: 18)
        case .idle:
            Circle()
                .fill(device.online ? Color(red: 0.20, green: 0.78, blue: 0.35) : Color(uiColor: .systemGray4))
                .frame(width: 9, height: 9)
                .frame(width: 18)
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
