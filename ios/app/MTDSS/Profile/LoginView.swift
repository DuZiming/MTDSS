/// 登录页(spec §4.6,mockup profile-pages ②;V3-1 四相位,android
/// LoginScreen 同象):
/// - **Form**:邮箱、验证码+「获取验证码」、蓝色登录大按钮;设备名默认
///   (平台名)作注脚不占表单位;「高级选项」小链接展开 服务器/CA(缺省
///   烤入 www.mtdss.top);被移除/轮询终态以警示条呈现;
/// - **Approval**:等待受信终端确认(对端弹窗)或 6 位审批码直输,轮询
///   后台收口;取消登录回退;
/// - **Limit**:踢选屏——设备数量已达上限,移出所选设备腾位,本机即登录;
///   退出登录回退;
/// - Done:导航翻转(权威态在 SessionModel,succeeded 一次性信号)。
/// 状态面全在 Kit ``LoginModel``(VM 测试钉契约帧;本视图纯呈现)。

import MTDSSKit
import SwiftUI

struct LoginView: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(SessionModel.self) private var session
    @State private var showAdvanced = false

    /// @Observable 模型以 @State 持有(生命周期随页;StateObject 是
    /// ObservableObject 面,不适用于 @Observable)。
    @State private var login: LoginModel

    init(repo: MtdssRepository, model: AppModel) {
        // App 装配:onLogin → 会话翻转 + status 重投影(selfDeviceId 随之)
        _login = State(initialValue: LoginModel(
            repo: repo,
            account: model.account,
            defaultDeviceName: UIDevice.current.name,
            onLogin: { result in
                model.session.applyLogin(result)
                Task { await model.refreshStatus() }
            },
        ))
    }

    var body: some View {
        VStack(spacing: 0) {
            ProfileNavBar(title: "登录", showsBack: true)
            Spacer().frame(height: 34)
            switch login.phase {
            case .form:
                formPanel
            case .approval:
                approvalPanel
            case .limit:
                limitPanel
            case .done:
                // 收口:onChange(succeeded) 随即弹回,不渲染
                Spacer()
            }
            Spacer()
            Text("MTDSS · 跨公网设备互传")
                .font(.system(size: 13))
                .foregroundStyle(Color(uiColor: .systemGray))
                .padding(.bottom, 26)
        }
        .background(Color(uiColor: .systemBackground))
        .toolbar(.hidden, for: .navigationBar)
        // 登录成功:权威态(SessionModel)已翻转,弹回我的页
        .onChange(of: login.succeeded) { _, value in
            if value != nil {
                dismiss()
            }
        }
    }

    // MARK: Form 相位(邮箱表单 + 被移除/终态警示条)

    private var formPanel: some View {
        VStack(spacing: 14) {
            if session.evicted {
                banner("本设备已被移除,请重新登录")
            }
            if let terminal = login.terminal {
                banner(terminal)
            }
            fields
            submitButton
            footnote
        }
        .padding(.horizontal, 20)
    }

    private var fields: some View {
        VStack(spacing: 12) {
            field {
                TextField("邮箱", text: $login.email)
                    .keyboardType(.emailAddress)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
            }
            field {
                TextField("验证码", text: $login.code)
                    .keyboardType(.numberPad)
                Divider()
                Button {
                    Task { await login.requestCode() }
                } label: {
                    if login.busy {
                        ProgressView().controlSize(.small)
                    } else {
                        Text("获取验证码")
                            .font(.system(size: 14.5, weight: .semibold))
                    }
                }
                .disabled(login.busy || login.email.trimmingCharacters(in: .whitespaces).isEmpty)
            }
            if showAdvanced {
                field {
                    TextField("服务器(默认 https://www.mtdss.top)", text: $login.server)
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                }
                field {
                    TextField("CA 证书路径(可选)", text: $login.caPath)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                }
            }
        }
    }

    /// 灰底圆角输入条(mockup .field:46pt 高,radius 11)。
    private func field<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        HStack(spacing: 0) {
            content()
                .font(.system(size: 16))
        }
        .padding(.horizontal, 14)
        .frame(height: 46)
        .frame(maxWidth: .infinity)
        .background(Color(uiColor: .systemGray6), in: RoundedRectangle(cornerRadius: 11))
    }

    private var submitButton: some View {
        Button {
            Task { await login.verify() }
        } label: {
            HStack(spacing: 8) {
                if login.busy {
                    ProgressView().tint(.white)
                }
                Text(login.busy ? "请稍候…" : "登录")
            }
            .font(.system(size: 17, weight: .semibold))
            .foregroundStyle(.white)
            .frame(maxWidth: .infinity)
            .frame(height: 50)
            .background(
                login.canSubmit ? Color.accentColor : Color.accentColor.opacity(0.4),
                in: RoundedRectangle(cornerRadius: 14),
            )
        }
        .disabled(!login.canSubmit)
    }

    // MARK: 注脚(设备名缺省 + 高级选项 + 错误)

    private var footnote: some View {
        VStack(spacing: 10) {
            if let error = login.errorText {
                Text(error)
                    .font(.system(size: 13))
                    .foregroundStyle(.red)
                    .multilineTextAlignment(.center)
            }
            Text("设备名将默认使用「\(login.effectiveDeviceName)」,可在 设置 中修改")
                .font(.system(size: 12.5))
                .foregroundStyle(Color(uiColor: .systemGray))
                .multilineTextAlignment(.center)
            Button {
                withAnimation { showAdvanced.toggle() }
            } label: {
                Text(showAdvanced ? "收起高级选项" : "高级选项(服务器 / CA 证书)")
                    .font(.system(size: 13))
                    .foregroundStyle(Color.accentColor)
            }
        }
    }

    // MARK: Approval 相位(等待受信终端确认 / 审批码直输)

    private var approvalPanel: some View {
        VStack(spacing: 14) {
            panelHeadline("等待审批", subtitle: "请在任一已登录的设备上确认本机登录")
            field {
                TextField("审批码", text: $login.approvalCode)
                    .keyboardType(.numberPad)
            }
            primaryButton("提交审批码", busy: login.busy) {
                Task { await login.submitApprovalCode() }
            }
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("等待确认中…")
                    .font(.system(size: 14))
                    .foregroundStyle(Color(uiColor: .systemGray))
                Spacer()
            }
            if let error = login.errorText {
                Text(error)
                    .font(.system(size: 13))
                    .foregroundStyle(.red)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            HStack {
                Spacer()
                Button("取消登录") {
                    login.cancel()
                }
                .font(.system(size: 14))
                .foregroundStyle(Color.accentColor)
                .disabled(login.busy)
            }
        }
        .padding(.horizontal, 20)
    }

    // MARK: Limit 相位(踢选屏:移出所选设备腾位)

    private var limitPanel: some View {
        VStack(spacing: 14) {
            panelHeadline(
                "设备数量已达上限",
                subtitle: "请选择一台设备移出账号,腾出名额后本机即登录",
            )
            ForEach(login.evictDevices, id: \.deviceId) { dev in
                evictRow(dev)
            }
            if let error = login.errorText {
                Text(error)
                    .font(.system(size: 13))
                    .foregroundStyle(.red)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            HStack {
                Spacer()
                Button("退出登录") {
                    login.cancel()
                }
                .font(.system(size: 14))
                .foregroundStyle(Color.accentColor)
                .disabled(login.busy)
            }
        }
        .padding(.horizontal, 20)
    }

    /// 踢选设备行(android LimitPanel 同构):名称 + 平台 · 在线/离线 + 移出。
    private func evictRow(_ dev: DirectoryDevice) -> some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(dev.name ?? dev.deviceId)
                    .font(.system(size: 16))
                    .foregroundStyle(Color.primary)
                    .lineLimit(1)
                Text([dev.platform, dev.online ? "在线" : "离线"].compactMap { $0 }.joined(separator: " · "))
                    .font(.system(size: 12.5))
                    .foregroundStyle(Color(uiColor: .systemGray))
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
            Button {
                Task { await login.evict(deviceId: dev.deviceId) }
            } label: {
                Text(login.busy ? "请稍候…" : "移出")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(.red)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 7)
                    .background(Color.red.opacity(0.1), in: Capsule())
            }
            .disabled(login.busy)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(Color(uiColor: .systemGray6), in: RoundedRectangle(cornerRadius: 11))
    }

    // MARK: 共用件

    /// 相位标题 + 副题(android headline/body 同词)。
    private func panelHeadline(_ title: String, subtitle: String) -> some View {
        VStack(spacing: 8) {
            Text(title)
                .font(.system(size: 24, weight: .semibold))
                .frame(maxWidth: .infinity, alignment: .leading)
            Text(subtitle)
                .font(.system(size: 14))
                .foregroundStyle(Color(uiColor: .systemGray))
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    /// 蓝色大按钮(表单主动作;审批码提交共用)。
    private func primaryButton(_ title: String, busy: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 8) {
                if busy {
                    ProgressView().tint(.white)
                }
                Text(title)
            }
            .font(.system(size: 17, weight: .semibold))
            .foregroundStyle(.white)
            .frame(maxWidth: .infinity)
            .frame(height: 50)
            .background(
                busy ? Color.accentColor.opacity(0.4) : Color.accentColor,
                in: RoundedRectangle(cornerRadius: 14),
            )
        }
        .disabled(busy)
    }

    /// 警示条(被移除/轮询终态;红底淡色圆角,android errorContainer 同 spirit)。
    private func banner(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 13.5))
            .foregroundStyle(.red)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .background(Color.red.opacity(0.1), in: RoundedRectangle(cornerRadius: 11))
    }
}
