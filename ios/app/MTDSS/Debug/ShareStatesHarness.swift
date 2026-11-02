/// 分享扩展状态页(DEBUG 截图面,R6 #4a;真机/产线不编译,特此报备):
/// 模拟器无法驱动真分享面板(需宿主 App 分享流),扩展面板的设备行三态
/// (idle 状态点 / busy spinner / launched 绿勾)+ 降级注记经**共享件**
/// ``ShareDeviceRow``/``ShareRowState`` 与扩展同源渲染(单文件双目标,
/// 防两处漂移)。截图:`MTDSS_DEBUG_TAB=transfers MTDSS_DEBUG_SHARESTATES=1`。
/// 静态数据占位(非真设备;仅渲染形态)。

#if DEBUG
import MTDSSKit
import SwiftUI

struct ShareStatesHarness: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("发送到 MTDSS(扩展状态页 · 共享件同源)")
                .font(.system(size: 15, weight: .semibold))
                .padding(.horizontal, 20)
                .padding(.top, 14)
                .padding(.bottom, 4)
            VStack(spacing: 0) {
                ShareDeviceRow(
                    device: SharedDeviceEntry(deviceId: "harness-1", name: "DESKTOP-VG14605", platform: "windows", online: true),
                    sub: "Windows · 在线",
                    enabled: true,
                    state: .idle,
                    onPick: {},
                )
                ShareDeviceRow(
                    device: SharedDeviceEntry(deviceId: "harness-2", name: "DESKTOP-VG14605", platform: "windows", online: true),
                    sub: "Windows · 在线",
                    enabled: false,
                    state: .busy,
                    onPick: {},
                )
                ShareDeviceRow(
                    device: SharedDeviceEntry(deviceId: "harness-3", name: "DESKTOP-VG14605", platform: "windows", online: true),
                    sub: "Windows · 已发送",
                    enabled: false,
                    state: .launched,
                    onPick: {},
                )
                ShareDeviceRow(
                    device: SharedDeviceEntry(deviceId: "harness-4", name: "MIX 3", platform: "android", online: false),
                    sub: "Android · 离线",
                    enabled: false,
                    state: .idle,
                    onPick: {},
                )
            }
            // 降级注记形(queued 相位注记,扩展面板同文)
            VStack(spacing: 10) {
                Text("已排队,打开 MTDSS 即自动发送")
                    .font(.system(size: 14, weight: .medium))
                Text("好")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(.white)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 12)
                    .background(Color.accentColor, in: RoundedRectangle(cornerRadius: 12))
            }
            .padding(.horizontal, 20)
            .padding(.top, 18)
            Spacer()
        }
        .background(Color(uiColor: .systemBackground))
    }
}
#endif
