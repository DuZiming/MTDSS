/// 登录页(spec §4.6,mockup profile-pages ②):极简三件——邮箱、验证码
/// +「获取验证码」、蓝色登录大按钮;设备名默认(平台名)作注脚不占表单位;
/// 「高级选项」小链接展开 服务器/CA(缺省烤入 www.mtdss.top)。
/// 状态面全在 Kit ``LoginModel``(VM 测试钉契约帧;本视图纯呈现)。

import MTDSSKit
import SwiftUI

struct LoginView: View {
    @Environment(\.dismiss) private var dismiss
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
            fields
            submitButton
            footnote
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

    // MARK: 表单

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
        .padding(.horizontal, 20)
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
                Text("登录")
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
        .padding(.horizontal, 20)
        .padding(.top, 14)
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
        .padding(.top, 14)
        .padding(.horizontal, 20)
    }
}
