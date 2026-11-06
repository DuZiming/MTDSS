/// 四标签壳(spec §4 总范式:四标签 + 悬浮玻璃胶囊标签栏;iOS 26 TabView
/// 原生 Liquid Glass 胶囊面,零自绘)。T4 只做空壳:每标签一枚占位视图,
/// 浏览(T5)/设备(T7)/传输(T9)/我的(T14+)由后续任务填充。

import SwiftUI

/// 标签枚举(计划 T4 接口面:四标签壳供后续任务填充)。
enum AppTab: String, CaseIterable, Identifiable {
    case favorites
    case devices
    case transfers
    case profile

    var id: String { rawValue }

    var title: String {
        switch self {
        case .favorites: "快捷目录"
        case .devices: "设备"
        case .transfers: "传输列表"
        case .profile: "我的"
        }
    }

    var symbol: String {
        switch self {
        case .favorites: "folder"
        case .devices: "display"
        case .transfers: "arrow.left.arrow.right"
        case .profile: "person"
        }
    }
}

struct AppTabView: View {
    @Environment(AppModel.self) private var model
    @State private var selection: AppTab = .favorites

    var body: some View {
        TabView(selection: $selection) {
            ForEach(AppTab.allCases) { tab in
                content(tab)
                    .tabItem { Label(tab.title, systemImage: tab.symbol) }
                    .tag(tab)
            }
        }
        .overlay(alignment: .top) {
            if let error = model.bootError {
                Text(error)
                    .font(.footnote)
                    .foregroundStyle(.white)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(.red.opacity(0.85), in: Capsule())
                    .padding(.top, 4)
            } else if model.engineRestarting {
                // 信任目录变更的引擎重启面(Ruling L,~1-2s;低频可接受)
                HStack(spacing: 8) {
                    ProgressView()
                        .controlSize(.small)
                    Text("重连中…")
                        .font(.footnote)
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
                .background(.blue.opacity(0.85), in: Capsule())
                .padding(.top, 4)
            }
        }
        // 受信终端审批弹窗(V3-1):任意屏面期间 event.approval 到达即弹——
        // 候选名/平台 + 审批码 + 同意/拒绝;「稍后」不应答(悬置会话由服务端
        // TTL 收口);应答在飞两键置灰,失败弹层内呈错误可重试。
        .alert(
            "新终端登录请求",
            isPresented: .init(
                get: { model.pendingApproval != nil },
                set: { shown in
                    if !shown {
                        // 应答在飞/失败待重试:不收弹层(成功/稍后按钮自行收口;
                        // 失败时弹层随错误文案重现)
                        if !model.approvalResponding && model.approvalRespondError == nil {
                            model.dismissApproval()
                        }
                    }
                },
            ),
            presenting: model.pendingApproval,
        ) { req in
            Button {
                Task { await model.respondApproval(req, allow: true) }
            } label: {
                model.approvalResponding ? Text("请稍候…") : Text("同意")
            }
            .disabled(model.approvalResponding)
            Button(role: .destructive) {
                Task { await model.respondApproval(req, allow: false) }
            } label: {
                model.approvalResponding ? Text("请稍候…") : Text("拒绝")
            }
            .disabled(model.approvalResponding)
            Button("稍后", role: .cancel) {
                model.dismissApproval()
            }
        } message: { req in
            Text(approvalMessage(req, error: model.approvalRespondError))
        }
        #if DEBUG
        .task {
            // MTDSS_DEBUG_TAB=<tab>:冒烟/截图钩子切标签(模拟器无触点驱动;
            // 真机/产线不编译,特此报备)
            if let tab = ProcessInfo.processInfo.environment["MTDSS_DEBUG_TAB"],
               let target = AppTab(rawValue: tab) {
                selection = target
            }
        }
        #endif
    }

    /// 审批弹窗文案(android ApprovalSheet 同词):候选名/平台 + 审批码;
    /// 应答失败追加错误行(可重试)。
    private func approvalMessage(_ req: AppModel.ApprovalRequest, error: String?) -> String {
        var text = "「\(req.candidateName)」请求登录你的账号"
        if let platform = req.candidatePlatform {
            text += "\n平台:\(platform)"
        }
        text += "\n\n审批码 \(req.code)"
        if let error {
            text += "\n\n\(error)"
        }
        return text
    }

    /// 各标签内容:快捷目录 = 授权目录列表根(T5);设备 = 设备列表根(T7);
    /// 传输 = 传输列表页(T9);我的 = 个人页(T10:登录/设置/信任目录)。
    @ViewBuilder
    private func content(_ tab: AppTab) -> some View {
        switch tab {
        case .favorites:
            FavoritesView()
        case .devices:
            DevicesView()
        case .transfers:
            #if DEBUG
            if ProcessInfo.processInfo.environment["MTDSS_DEBUG_SHARESTATES"] == "1" {
                // 分享扩展状态页(DEBUG 截图面;sim 无真分享面板入口)
                ShareStatesHarness()
            } else {
                TransfersView()
            }
            #else
            TransfersView()
            #endif
        case .profile:
            ProfileView()
        }
    }
}
