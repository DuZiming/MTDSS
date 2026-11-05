/// 设置页(spec §4.6,mockup profile-pages ③):三块——
/// - 我的设备名(默认主机名/平台名,点入改;无运行时改名契约面 → App 侧
///   改名,**下次登录对端生效**,行下注脚如实);
/// - 显示隐藏项(iOS 开关,默认关;App 侧 UserDefaults,浏览面消费);
/// - 允许删除远端文件(R9 #3,默认关;远端浏览删除动作的 App 侧门);
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
    /// 照片图库授权态(R7:进页现拉 + 变化流活体跟随;本段 = 权限入口,
    /// 非信任目录条目——照片图库是其他设备可读的特殊来源,权限 = 数据面
    /// 开关)。
    @State private var photoPermission: PhotoLibraryPermission?
    /// PHOTO_PROMPT 钩子已执行闸(sim 授权弹窗应答测试;恰一次)。
    @State private var photoPromptDone = false

    var body: some View {
        VStack(spacing: 0) {
            ProfileNavBar(title: "设置", showsBack: true)
            SinkingFooterList {
                VStack(spacing: 0) {
                    identitySection
                    sectionGap
                    photoSection
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
        // 移除确认(R5b #9:删除类确认一律居中 alert——退出登录 R5a #10/
        // 浏览删除同款;底部 action sheet 样式废止)
        .alert(
            "移除「\(pendingRemove?.name ?? "")」?",
            isPresented: .init(
                get: { pendingRemove != nil },
                set: { if !$0 { pendingRemove = nil } },
            ),
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
        .task {
            await autoTrustIfHooked()
            // 照片授权态进页现拉 + 变化流活体跟随(权限本体归系统,本页
            // 只呈现 + 请求入口/设置链接)。PROMPT 钩子先于长驻消费(恰
            // 一次触发,弹窗由 GrantPhotosUITests 应答)。
            photoPermission = await PHPhotoLibraryService.shared.permissionStatus()
            await photoPromptIfHooked()
            for await status in PHPhotoLibraryService.shared.permissionChanges() {
                photoPermission = status
            }
        }
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
            // 允许删除远端文件(R9 #3 安全门,默认关):与显示隐藏项同段同形
            // (两个浏览面开关);门收 App UI 面——远端浏览行滑删/选择删除
            // 显隐,本地删除不设门。
            FullBleedRow {
                HStack(spacing: 10) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("允许删除远端文件")
                            .font(.system(size: 17))
                            .foregroundStyle(Color.primary)
                        Text("开启后,浏览其他设备时可对文件执行删除")
                            .font(.system(size: 12.5))
                            .foregroundStyle(Color(uiColor: .systemGray))
                    }
                    Spacer(minLength: 0)
                    Toggle("", isOn: Binding(
                        get: { model.allowRemoteDelete },
                        set: { model.allowRemoteDelete = $0 },
                    ))
                    .labelsHidden()
                    .accessibilityIdentifier("allow-remote-delete-toggle")
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

    // MARK: 照片图库块(R7 Ruling P:权限入口——照片图库 = 其他设备可读
    //        的特殊来源,授权即数据面开关;非信任目录条目)

    private var photoSection: some View {
        VStack(spacing: 0) {
            Text("照片图库")
                .font(.system(size: 13))
                .foregroundStyle(Color(uiColor: .systemGray))
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 20)
                .padding(.top, 12)
                .padding(.bottom, 6)

            FullBleedRow {
                Button {
                    switch photoPermission {
                    case .notDetermined, .none:
                        Task { await requestPhotoAccess() }
                    default:
                        openSystemSettings()
                    }
                } label: {
                    HStack(spacing: 13) {
                        Image(systemName: "photo.stack")
                            .font(.system(size: 20))
                            .foregroundStyle(Color.accentColor)
                            .frame(width: 28)
                        VStack(alignment: .leading, spacing: 2) {
                            Text("照片图库权限")
                                .font(.system(size: 16.5))
                                .foregroundStyle(Color.primary)
                                .lineLimit(1)
                            Text(Self.photoStatusText(photoPermission))
                                .font(.system(size: 12.5))
                                .foregroundStyle(Color(uiColor: .systemGray))
                        }
                        Spacer(minLength: 0)
                        // 未询问 = 行内「请求授权」;其余态 = 系统设置深链
                        switch photoPermission {
                        case .notDetermined, .none:
                            EmptyView()
                        default:
                            Text("›")
                                .font(.system(size: 17))
                                .foregroundStyle(Color.accentColor)
                        }
                    }
                    .padding(.horizontal, 20)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Text(photoPermission == .notDetermined ? "请求照片权限" : "照片图库权限"))
            }

            // 撤权/拒绝 = 指引文案(与信任目录「移除」同一 spirit:数据面
            // 关闭的恢复路径;深链在行点按)
            Text(Self.photoFooterText(photoPermission))
                .font(.system(size: 12.5))
                .foregroundStyle(Color(uiColor: .systemGray))
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 20)
                .padding(.vertical, 8)
        }
    }

    /// 请求授权(行内入口;结果落状态面,变化流活体跟随)。
    private func requestPhotoAccess() async {
        photoPermission = await PHPhotoLibraryService.shared.requestAccess()
    }

    /// 系统设置深链(R7 撤 PhotoLibraryView 后收编本页:授权/撤销恢复
    /// 路径的入口,R5c `PhotoLibraryView.openSystemSettings` 同实现)。
    private func openSystemSettings() {
        guard let url = URL(string: UIApplication.openSettingsURLString) else { return }
        UIApplication.shared.open(url)
    }

    /// 授权态文案(未查询 = 转圈期占位空串)。
    private static func photoStatusText(_ p: PhotoLibraryPermission?) -> String {
        switch p {
        case .none: ""
        case .notDetermined: "未询问"
        case .denied: "未授权"
        case .authorized: "已授权"
        case .limited: "部分照片"
        }
    }

    /// 段脚注:按态指引(未询问 = 授权即允许其他设备读取;拒绝 = 前往
    /// 设置恢复;授权 = 撤销路径)。
    private static func photoFooterText(_ p: PhotoLibraryPermission?) -> String {
        switch p {
        case .none, .notDetermined:
            "点「请求授权」允许其他设备读取你的照片图库并下载照片"
        case .denied:
            "已拒绝。前往系统设置 > Synshu > 照片可恢复"
        case .authorized:
            "已授权:其他设备可浏览与下载你的照片\n撤销:系统设置 > Synshu > 照片"
        case .limited:
            "已允许所选照片;系统设置 > Synshu > 照片可调整"
        }
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
        return "Synshu \(version)"
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

    /// MTDSS_DEBUG_PHOTO_PROMPT=1:进页即发照片授权请求(R7 钦定入口 =
    /// 设置页;弹窗由 GrantPhotosUITests springboard 应答铸 TCC 授权行
    /// ——R5c 同法,simctl privacy grant 在 iOS 26 sim 不被 tccd 采信)。
    private func photoPromptIfHooked() async {
        #if DEBUG
        guard !photoPromptDone,
              ProcessInfo.processInfo.environment["MTDSS_DEBUG_PHOTO_PROMPT"] == "1" else { return }
        photoPromptDone = true
        await requestPhotoAccess()
        #endif
    }
}
