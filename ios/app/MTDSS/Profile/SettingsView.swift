/// 设置页(spec §4.6,mockup profile-pages ③):三块——
/// - 我的设备名(默认主机名/平台名,点入改;无运行时改名契约面 → App 侧
///   改名,**下次登录对端生效**,行下注脚如实);
/// - 显示隐藏项(iOS 开关,默认关;App 侧 UserDefaults,浏览面消费);
/// - 信任目录(已授权列表 + 移除;「添加…」开系统目录选择器即授权 = 书签
///   入账 + engine-roots.conf 重写 + 引擎重启;小节注明「收件箱默认可见,
///   无需添加」)。

import MTDSSKit
import SwiftUI
import UniformTypeIdentifiers

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(AccountStore.self) private var account
    @Environment(TrustedRootsStore.self) private var trusted
    @Environment(\.dismiss) private var dismiss

    @State private var showRename = false
    @State private var renameText = ""
    @State private var showImporter = false
    @State private var pendingRemove: TrustedRoot?
    @State private var trustError: String?
    /// AUTOTRUST 钩子已执行闸(sim 冒烟;每次呈现恰一次)。
    @State private var autoTrustDone = false

    var body: some View {
        VStack(spacing: 0) {
            ProfileNavBar(title: "设置", showsBack: true)
            SinkingFooterList {
                VStack(spacing: 0) {
                    identitySection
                    sectionGap
                    trustSection
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
        .alert("我的设备名", isPresented: $showRename) {
            TextField("设备名", text: $renameText)
            Button("保存") {
                let name = renameText.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !name.isEmpty else { return }
                account.setDeviceNameOverride(name)
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("修改后将在下次登录时对其他设备生效")
        }
        .confirmationDialog(
            "移除「\(pendingRemove?.name ?? "")」?",
            isPresented: .init(
                get: { pendingRemove != nil },
                set: { if !$0 { pendingRemove = nil } },
            ),
            titleVisibility: .visible,
        ) {
            Button("移除", role: .destructive) {
                if let root = pendingRemove {
                    Task {
                        await model.removeTrustedRoot(root)
                    }
                }
                pendingRemove = nil
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("其他设备将无法再浏览该目录")
        }
        .fileImporter(
            isPresented: $showImporter,
            allowedContentTypes: [.folder],
            allowsMultipleSelection: false,
        ) { result in
            guard case .success(let urls) = result, let url = urls.first else { return }
            Task {
                if let error = await model.addTrustedRoot(url: url) {
                    trustError = error
                }
            }
        }
        .alert("添加信任目录失败", isPresented: .init(
            get: { trustError != nil },
            set: { if !$0 { trustError = nil } },
        )) {
            Button("好", role: .cancel) {}
        } message: {
            Text(trustError ?? "")
        }
        .task { await autoTrustIfHooked() }
    }

    // MARK: 身份块(设备名 + 显示隐藏项)

    private var identitySection: some View {
        VStack(spacing: 0) {
            FullBleedRow {
                Button {
                    renameText = account.effectiveDeviceName(default: "")
                    showRename = true
                } label: {
                    HStack(spacing: 10) {
                        Text("我的设备名")
                            .font(.system(size: 17))
                            .foregroundStyle(Color.primary)
                        Spacer(minLength: 0)
                        // 生效名 = 账本(改名/登录实名)> 平台名(spec:默认主机名)
                        Text(account.effectiveDeviceName(default: UIDevice.current.name))
                            .font(.system(size: 12.5))
                            .foregroundStyle(Color(uiColor: .systemGray))
                            .lineLimit(1)
                        Text("›")
                            .font(.system(size: 17))
                            .foregroundStyle(Color.accentColor)
                    }
                    .padding(.horizontal, 20)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            FullBleedRow {
                HStack(spacing: 10) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("显示隐藏项")
                            .font(.system(size: 17))
                            .foregroundStyle(Color.primary)
                        Text("浏览时显示隐藏目录与文件")
                            .font(.system(size: 12.5))
                            .foregroundStyle(Color(uiColor: .systemGray))
                    }
                    Spacer(minLength: 0)
                    Toggle("", isOn: Binding(
                        get: { model.showHidden },
                        set: { model.showHidden = $0 },
                    ))
                    .labelsHidden()
                }
                .padding(.horizontal, 20)
            }
            // 改名生效面注脚(无运行时契约面,如实呈现)
            Text("设备名修改将在下次登录时对其他设备生效")
                .font(.system(size: 12.5))
                .foregroundStyle(Color(uiColor: .systemGray))
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 20)
                .padding(.vertical, 8)
        }
    }

    private var sectionGap: some View {
        Rectangle()
            .fill(Color(uiColor: .systemGray5))
            .frame(height: 8)
    }

    // MARK: 信任目录块

    private var trustSection: some View {
        VStack(spacing: 0) {
            Text("信任目录(其他设备可浏览)")
                .font(.system(size: 13))
                .foregroundStyle(Color(uiColor: .systemGray))
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 20)
                .padding(.top, 12)
                .padding(.bottom, 6)

            if model.engineRestarting {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("重连中…")
                        .font(.system(size: 14))
                        .foregroundStyle(Color(uiColor: .systemGray))
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 20)
                .padding(.vertical, 14)
            }

            ForEach(trusted.roots) { root in
                FullBleedRow {
                    HStack(spacing: 13) {
                        RootFolderIcon()
                        VStack(alignment: .leading, spacing: 2) {
                            Text(root.name)
                                .font(.system(size: 16.5))
                                .foregroundStyle(Color.primary)
                                .lineLimit(1)
                            Text("已授权")
                                .font(.system(size: 12.5))
                                .foregroundStyle(Color(uiColor: .systemGray))
                        }
                        Spacer(minLength: 0)
                        Button {
                            pendingRemove = root
                        } label: {
                            Text("移除")
                                .font(.system(size: 13))
                                .foregroundStyle(.red)
                                .padding(.horizontal, 8)
                                .padding(.vertical, 6)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .disabled(model.engineRestarting)
                    }
                    .padding(.horizontal, 20)
                }
            }

            FullBleedRow {
                Button {
                    showImporter = true
                } label: {
                    HStack(spacing: 13) {
                        Image(systemName: "plus")
                            .font(.system(size: 17, weight: .medium))
                            .foregroundStyle(Color.accentColor)
                            .frame(width: 22)
                        Text("添加信任目录…")
                            .font(.system(size: 17))
                            .foregroundStyle(Color.accentColor)
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, 20)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(model.engineRestarting)
            }

            Text("点「添加」打开系统目录选择器,选定即授权\n收件箱默认对其他设备可见,无需添加")
                .font(.system(size: 12.5))
                .foregroundStyle(Color(uiColor: .systemGray))
                .multilineTextAlignment(.center)
                .frame(maxWidth: .infinity)
                .padding(.top, 18)
        }
    }

    private var appVersion: String {
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?"
        return "MTDSS \(version)"
    }

    // MARK: DEBUG 冒烟钩子(模拟器无触点驱动;真机/产线不编译,特此报备)

    /// MTDSS_DEBUG_AUTOTRUST=<path>:出现即走「选择器回调」同码路径添加
    /// (书签→conf→引擎重启);MTDSS_DEBUG_UNTRUST=<path>:同码路径移除
    /// (按授权快照路径匹配)。文件选择器本身无 simctl 触点,故走此钩子;
    /// 先等引擎 boot 就绪(repository 就位,重启面才能立即生效)。
    private func autoTrustIfHooked() async {
        #if DEBUG
        guard !autoTrustDone else { return }
        let env = ProcessInfo.processInfo.environment
        let addPath = env["MTDSS_DEBUG_AUTOTRUST"].flatMap { $0.isEmpty ? nil : $0 }
        let untrustPath = env["MTDSS_DEBUG_UNTRUST"].flatMap { $0.isEmpty ? nil : $0 }
        guard addPath != nil || untrustPath != nil else { return }
        autoTrustDone = true
        for _ in 0..<100 where model.repository == nil {
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        if let addPath {
            if let error = await model.addTrustedRoot(url: URL(fileURLWithPath: addPath)) {
                trustError = error
            }
        } else if let untrustPath,
                  let root = trusted.roots.first(where: { $0.path == untrustPath }) {
            await model.removeTrustedRoot(root)
        }
        #endif
    }
}
