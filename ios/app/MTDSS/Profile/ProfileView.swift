/// 我的页(spec §4.6,mockup profile-pages ①):
/// - 左上扫码图标(占位「敬请期待」)+ 右上齿轮(进设置);
/// - 账号行 = 字母头像 + 邮箱 + 「设备名 · 在线/离线」;未登录 = 一枚
///   「登录」按钮(推入登录页);
/// - 联系我们 / 为我打分 / 我的权益 三个占位行(› 进占位页);
/// - 退出登录红字行(确认后 login.logout;会话清理链在 SessionModel);
/// - 版本号沉底。

import MTDSSKit
import SwiftUI

struct ProfileView: View {
    @Environment(AppModel.self) private var model
    @Environment(SessionModel.self) private var session
    @Environment(AccountStore.self) private var account
    @State private var navPath: [ProfileRoute] = []
    @State private var showScanHint = false
    @State private var showLogoutConfirm = false
    @State private var logoutError: String?

    var body: some View {
        NavigationStack(path: $navPath) {
            rootPage
                .navigationDestination(for: ProfileRoute.self) { route in
                    destination(route)
                }
        }
        .task {
            await model.refreshStatus()
            // 会话面进页重拉(T10:boot 后登录动作早于订阅的竞窗兜底)
            if let repo = model.repository {
                await session.refresh(repo: repo)
            }
            await pushIfDebugHooked()
        }
        .alert("敬请期待", isPresented: $showScanHint) {
            Button("好", role: .cancel) {}
        } message: {
            Text("扫码功能将在后续版本提供")
        }
        .confirmationDialog("退出当前账号?", isPresented: $showLogoutConfirm, titleVisibility: .visible) {
            Button("退出登录", role: .destructive) {
                Task { await logout() }
            }
            Button("取消", role: .cancel) {}
        }
        .alert("登出失败", isPresented: .init(
            get: { logoutError != nil },
            set: { if !$0 { logoutError = nil } },
        )) {
            Button("好", role: .cancel) {}
        } message: {
            Text(logoutError ?? "")
        }
    }

    // MARK: 根页

    private var rootPage: some View {
        VStack(spacing: 0) {
            navRow
            SinkingFooterList {
                VStack(spacing: 0) {
                    accountSection
                    placeholderRows
                    if session.loggedIn == true {
                        logoutRow
                    }
                }
            } footer: {
                Text(appVersion)
                    .font(.system(size: 13))
                    .foregroundStyle(Color(uiColor: .systemGray))
                    .frame(maxWidth: .infinity)
                    .padding(.top, 10)
                    .padding(.bottom, 14)
            }
        }
        .background(Color(uiColor: .systemBackground))
        .toolbar(.hidden, for: .navigationBar)
    }

    /// 顶栏(mockup .navrow):左扫码(占位)+ 标题绝对居中 + 右齿轮。
    private var navRow: some View {
        ZStack {
            HStack {
                Button {
                    showScanHint = true
                } label: {
                    Image(systemName: "qrcode.viewfinder")
                        .font(.system(size: 20))
                        .foregroundStyle(Color.accentColor)
                        .frame(width: 34, height: 34)
                        .contentShape(Rectangle())
                }
                Spacer()
                Button {
                    navPath.append(.settings)
                } label: {
                    Image(systemName: "gearshape")
                        .font(.system(size: 20))
                        .foregroundStyle(Color.accentColor)
                        .frame(width: 34, height: 34)
                        .contentShape(Rectangle())
                }
            }
            Text("我的")
                .font(.system(size: 17, weight: .semibold))
                .padding(.horizontal, 100)
        }
        .padding(.horizontal, 18)
        .padding(.top, 10)
        .padding(.bottom, 8)
    }

    /// 账号区:已登录 = 字母头像+邮箱(或本机名;smoke/他端登录时引擎无
    /// email 面)+「设备名 · 在线/离线」;未登录 = 登录钮。
    @ViewBuilder
    private var accountSection: some View {
        if session.loggedIn == true {
            let title = account.email ?? effectiveName
            FullBleedRow(height: 74, separatorLeadingInset: 20) {
                HStack(spacing: 13) {
                    avatar(email: title)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(title)
                            .font(.system(size: 18))
                            .foregroundStyle(Color.primary)
                            .lineLimit(1)
                        Text("\(effectiveName) · \(session.online == true ? "在线" : "离线")")
                            .font(.system(size: 12.5))
                            .foregroundStyle(Color(uiColor: .systemGray))
                            .lineLimit(1)
                    }
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 20)
            }
            .accessibilityElement(children: .combine)
        } else {
            VStack(spacing: 0) {
                Button {
                    if model.repository != nil {
                        navPath.append(.login)
                    }
                } label: {
                    Text("登录")
                        .font(.system(size: 17, weight: .semibold))
                        .foregroundStyle(.white)
                        .frame(maxWidth: .infinity)
                        .frame(height: 50)
                        .background(Color.accentColor, in: RoundedRectangle(cornerRadius: 14))
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 14)
                .disabled(model.repository == nil)
            }
        }
    }

    /// 字母头像(邮箱首字符大写;蓝底白字圆)。
    private func avatar(email: String) -> some View {
        let letter = email.first.map { String($0).uppercased() } ?? "?"
        return Text(letter)
            .font(.system(size: 23, weight: .semibold))
            .foregroundStyle(.white)
            .frame(width: 52, height: 52)
            .background(Color.accentColor, in: Circle())
    }

    /// 三个占位行(spec:› 进占位页)。
    private var placeholderRows: some View {
        VStack(spacing: 0) {
            placeholderRow(.contact, icon: "envelope", title: "联系我们")
            placeholderRow(.rating, icon: "star", title: "为我打分")
            placeholderRow(.benefits, icon: "gift", title: "我的权益")
        }
    }

    private func placeholderRow(_ route: ProfileRoute, icon: String, title: String) -> some View {
        Button {
            navPath.append(route)
        } label: {
            FullBleedRow {
                HStack(spacing: 13) {
                    Image(systemName: icon)
                        .font(.system(size: 20))
                        .foregroundStyle(Color(uiColor: .systemGray))
                        .frame(width: 26)
                    Text(title)
                        .font(.system(size: 17))
                        .foregroundStyle(Color.primary)
                    Spacer(minLength: 0)
                    Text("›")
                        .font(.system(size: 17))
                        .foregroundStyle(Color.accentColor)
                }
                .padding(.horizontal, 20)
            }
        }
        .buttonStyle(.plain)
    }

    /// 退出登录红字行(居中;确认对话框)。
    private var logoutRow: some View {
        Button {
            showLogoutConfirm = true
        } label: {
            FullBleedRow {
                HStack {
                    Spacer()
                    Text("退出登录")
                        .font(.system(size: 17))
                        .foregroundStyle(.red)
                    Spacer()
                }
                .padding(.horizontal, 20)
            }
        }
        .buttonStyle(.plain)
        .padding(.top, 22)
    }

    // MARK: 动作

    private func logout() async {
        guard let repo = model.repository else { return }
        do {
            try await session.logout(repo: repo)
            await model.refreshStatus()
        } catch {
            logoutError = LoginModel.userErrorText(for: error)
        }
    }

    private var effectiveName: String {
        account.effectiveDeviceName(default: "本机")
    }

    private var appVersion: String {
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?"
        return "MTDSS \(version)"
    }

    // MARK: 推入页

    @ViewBuilder
    private func destination(_ route: ProfileRoute) -> some View {
        switch route {
        case .login:
            if let repo = model.repository {
                LoginView(repo: repo, model: model)
            } else {
                Text("引擎就绪中…")
            }
        case .settings:
            SettingsView()
        case .contact:
            PlaceholderPage(title: "联系我们", icon: "envelope")
        case .rating:
            PlaceholderPage(title: "为我打分", icon: "star")
        case .benefits:
            PlaceholderPage(title: "我的权益", icon: "gift")
        }
    }

    // MARK: DEBUG 冒烟钩子(模拟器无触点驱动;真机/产线不编译,特此报备)

    /// MTDSS_DEBUG_AUTOPUSH=login|settings:我的栈直接推入(截图用;
    /// 沿用 T5/T7 钩子语言)。login 推入前置 repository 就绪,故先等 boot。
    private func pushIfDebugHooked() async {
        #if DEBUG
        guard let spec = ProcessInfo.processInfo.environment["MTDSS_DEBUG_AUTOPUSH"],
              spec == "login" || spec == "settings" else { return }
        for _ in 0..<100 where model.repository == nil {
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        if navPath.isEmpty {
            navPath = [spec == "login" ? .login : .settings]
        }
        #endif
    }
}

/// 我的栈内路由(NavigationStack 值路由)。
enum ProfileRoute: Hashable {
    case login
    case settings
    case contact
    case rating
    case benefits
}

/// 占位页(联系我们/为我打分/我的权益;spec:› 进占位页)。
struct PlaceholderPage: View {
    let title: String
    let icon: String
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 0) {
            ProfileNavBar(title: title, showsBack: true)
            ContentUnavailableView(
                title,
                systemImage: icon,
                description: Text("敬请期待"),
            )
            .frame(maxHeight: .infinity)
        }
        .background(Color(uiColor: .systemBackground))
        .toolbar(.hidden, for: .navigationBar)
    }
}

/// 我的栈顶栏(推入页:返回 + 标题绝对居中;mockup .navrow 像素参照)。
/// 返回动作:外部给 onBack 用之,缺省弹栈(dismiss)。
struct ProfileNavBar: View {
    let title: String
    var showsBack: Bool = false
    var onBack: (() -> Void)? = nil
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        ZStack {
            HStack {
                if showsBack {
                    Button {
                        if let onBack {
                            onBack()
                        } else {
                            dismiss()
                        }
                    } label: {
                        Image(systemName: "chevron.left")
                            .font(.system(size: 16, weight: .bold))
                            .foregroundStyle(Color.accentColor)
                            .frame(width: 20, height: 30)
                            .contentShape(Rectangle())
                    }
                }
                Spacer()
            }
            Text(title)
                .font(.system(size: 17, weight: .semibold))
                .lineLimit(1)
                .padding(.horizontal, 100)
        }
        .padding(.horizontal, 18)
        .padding(.top, 10)
        .padding(.bottom, 8)
    }
}
