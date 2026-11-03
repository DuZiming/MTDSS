/// 分享扩展状态页(DEBUG 截图面,R6 #4a + R10 直发面;真机/产线不编译,
/// 特此报备):模拟器无法驱动真分享面板(需宿主 App 分享流),扩展面板的
/// 设备行三态(idle 状态点 / busy spinner / launched 绿勾)、**直发进度面**
/// (R10:逐文件行 + 总进度条 + 取消钮,``ShareSendRow``/``ShareOverallBar``
/// 共享件同源)与降级注记经共享件与扩展同源渲染(单文件双目标,防两处
/// 漂移)。截图:`MTDSS_DEBUG_TAB=transfers MTDSS_DEBUG_SHARESTATES=1`。
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
                    sub: "Windows · 准备中",
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
            // R10 直发进度面(逐文件行三态:完成/在途/待发)
            Text("直发进度(逐文件行)")
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(Color(uiColor: .systemGray))
                .padding(.horizontal, 20)
                .padding(.top, 14)
                .padding(.bottom, 2)
            VStack(spacing: 0) {
                ShareSendRow(file: .init(name: "IMG_2049.JPG", size: 3_800_000, bytesDone: 3_800_000))
                ShareSendRow(file: .init(name: "IMG_2050.JPG", size: 2_400_000, bytesDone: 960_000))
                ShareSendRow(file: .init(name: "报告.pdf", size: 512_000, bytesDone: 0))
            }
            // R10 总进度条 + 取消
            ShareOverallBar(
                progress: DirectSendProgress(
                    files: [
                        .init(name: "IMG_2049.JPG", size: 3_800_000, bytesDone: 3_800_000),
                        .init(name: "IMG_2050.JPG", size: 2_400_000, bytesDone: 960_000),
                        .init(name: "报告.pdf", size: 512_000, bytesDone: 0),
                    ],
                    filesDone: 1, bytesDone: 4_760_000, bytesTotal: 6_712_000),
                onCancel: {})
                .padding(.top, 14)
            // 降级注记形(queued 相位注记,扩展面板同文)
            VStack(spacing: 10) {
                Text("已排队,打开 MTDSS 即自动发送")
                    .font(.system(size: 14, weight: .medium))
            }
            .padding(.horizontal, 20)
            .padding(.top, 18)
            Spacer()
        }
        .background(Color(uiColor: .systemBackground))
    }
}
#endif
