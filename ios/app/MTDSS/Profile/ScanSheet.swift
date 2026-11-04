/// 扫码登录取景页(V3-1 扫码器点亮;我的页扫码占位接任):
/// - 相机会话 = AVCaptureMetadataOutput QR-only,页内 sheet(不开外链);
/// - 权限:进入页请求(.notDetermined → requestAccess);拒绝/受限 → 指引
///   文案 + 「前往设置」(照片权限同款 spirit);
/// - 命中链路:QR 原文交 Kit ``QrLoginCode`` 纯解析——命中本家形态即回调
///   token(claim/approve 授权闭环由调用方 ProfileView 经 Kit
///   ``QrApprovalModel`` 接线,本页不受影响);非本家码保持扫描,行内提示;
/// - 竖屏取景全幅 + 取景框角标 + 提示文案沉底(android 扫码屏同象)。

import AVFoundation
import MTDSSKit
import SwiftUI

struct ScanSheet: View {
    /// 命中 MTDSS 登录码回调(token;调用方接确认面)。
    let onCode: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    /// 相机授权态(nil = 未请求;notDetermined 请求中)。
    @State private var permission: AVAuthorizationStatus?
    /// 非本家码行内提示(命中本家码即回调离场)。
    @State private var hint: String?
    @State private var scanner = ScannerCore()

    var body: some View {
        VStack(spacing: 0) {
            ProfileNavBar(title: "扫码登录", showsBack: true)
            if let permission, permission == .denied || permission == .restricted {
                permissionDenied
            } else if let permission, permission == .authorized {
                ZStack {
                    ScannerPreview(scanner: scanner)
                        .ignoresSafeArea(edges: .bottom)
                    viewfinder
                }
                .frame(maxHeight: .infinity)
            } else {
                // 未授权/请求中
                Spacer()
                VStack(spacing: 12) {
                    ProgressView()
                    Text("正在请求相机权限…")
                        .font(.system(size: 14))
                        .foregroundStyle(Color(uiColor: .systemGray))
                }
                Spacer()
            }
            hintRow
        }
        .background(Color.black.opacity(0.92))
        .toolbar(.hidden, for: .navigationBar)
        .task {
            await ensurePermission()
        }
        .task {
            // QR 原文流消费(单订阅;视图终结即取消)
            for await payload in scanner.payloadStream {
                handlePayload(payload)
            }
        }
        .onChange(of: permission) { _, status in
            if status == .authorized {
                scanner.start()
            }
        }
        .onDisappear {
            scanner.stop()
        }
    }

    // MARK: 取景覆盖件

    /// 取景框角标(四角 L 形;居中 240×240)。
    private var viewfinder: some View {
        RoundedRectangle(cornerRadius: 18)
            .stroke(Color.white.opacity(0.9), lineWidth: 2)
            .frame(width: 240, height: 240)
            .overlay(alignment: .bottom) {
                Text("将 PC 端「扫码登录」二维码放入取景框")
                    .font(.system(size: 13.5))
                    .foregroundStyle(.white.opacity(0.9))
                    .padding(.top, 14)
                    .offset(y: 34)
            }
    }

    /// 行内提示(非本家码/通用引导;沉底)。
    private var hintRow: some View {
        Group {
            if let hint {
                Text(hint)
                    .font(.system(size: 13.5))
                    .foregroundStyle(.orange)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 20)
                    .padding(.vertical, 10)
            } else {
                Text("扫码登录 PC 端(MTDSS 桌面「扫码登录」入口)")
                    .font(.system(size: 13.5))
                    .foregroundStyle(.white.opacity(0.75))
                    .padding(.horizontal, 20)
                    .padding(.vertical, 10)
            }
        }
        .padding(.bottom, 18)
    }

    // MARK: 权限拒指引(照片权限同款 spirit:指路设置页,不重复弹)

    private var permissionDenied: some View {
        VStack(spacing: 14) {
            Spacer()
            Image(systemName: "camera.badge.ellipsis")
                .font(.system(size: 44))
                .foregroundStyle(Color(uiColor: .systemGray))
            Text("未获得相机权限")
                .font(.system(size: 17, weight: .semibold))
                .foregroundStyle(.white)
            Text("扫码登录需要使用相机。\n请在系统设置中允许 MTDSS 访问相机后重试。")
                .font(.system(size: 13.5))
                .foregroundStyle(Color(uiColor: .systemGray))
                .multilineTextAlignment(.center)
            Button {
                if let url = URL(string: UIApplication.openSettingsURLString) {
                    UIApplication.shared.open(url)
                }
            } label: {
                Text("前往设置")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 22)
                    .padding(.vertical, 10)
                    .background(Color.accentColor, in: Capsule())
            }
            .padding(.top, 4)
            Spacer()
        }
        .padding(.horizontal, 30)
    }

    // MARK: 命中处理

    private func ensurePermission() async {
        let status = AVCaptureDevice.authorizationStatus(for: .video)
        if status == .notDetermined {
            let granted: AVAuthorizationStatus = await withCheckedContinuation { cont in
                AVCaptureDevice.requestAccess(for: .video) { granted in
                    cont.resume(returning: granted ? .authorized : .denied)
                }
            }
            permission = granted
        } else {
            permission = status
        }
    }

    /// QR 原文 → Kit 纯解析:命中本家形态回调 token 离场;否则行内提示续扫。
    private func handlePayload(_ payload: String) {
        if let token = QrLoginCode.parse(payload) {
            scanner.stop()
            onCode(token)
            dismiss()
        } else {
            hint = "非 MTDSS 登录码,请对准 PC 端「扫码登录」二维码"
        }
    }
}

// MARK: 相机会话核心(App 侧;UIKit/AVFoundation 不进 Kit)

/// QR 扫描核心:会话配置一次,起停幂等;元数据回调经 payloadStream(Ayield
/// 帧流,ScanSheet onReceive 消费)转主线程。
@MainActor
final class ScannerCore: NSObject, AVCaptureMetadataOutputObjectsDelegate {
    let session = AVCaptureSession()
    /// 扫得的 QR 原文流(AsyncStream;主线程投递,慢消费丢旧帧)。
    let payloadStream: AsyncStream<String>
    private let payloadContinuation: AsyncStream<String>.Continuation
    private let sessionQueue = DispatchQueue(label: "top.mtdss.ios.scan")
    private var configured = false
    /// 元数据输出(configureSession 挂上后记下):capture output 对 delegate
    /// **强持有**(AVFoundation 例外,非常规弱引用惯例),stop() 不摘则
    /// ScannerCore→session→output→delegate 环不破,取景页销毁后整组滞留。
    private var output: AVCaptureMetadataOutput?

    override init() {
        (payloadStream, payloadContinuation) = AsyncStream.makeStream(
            of: String.self,
            bufferingPolicy: .bufferingNewest(1),
        )
        super.init()
    }

    /// 配置(一次)+ 启动(幂等;相机不可得静默——模拟器无摄像头,真机才有效)。
    /// delegate 每次启动重挂(stop 摘环后 start 复活,起停幂等承诺保持)。
    func start() {
        if !configured {
            configured = true
            configureSession()
        }
        output?.setMetadataObjectsDelegate(self, queue: sessionQueue)
        sessionQueue.async { [session] in
            if !session.isRunning {
                session.startRunning()
            }
        }
    }

    func stop() {
        // 摘 delegate 破保留环(T13 review minor ②):output 强持有 delegate,
        // 不清则 ScannerCore↔session 互锁滞留;start 重新挂回。
        output?.setMetadataObjectsDelegate(nil, queue: nil)
        sessionQueue.async { [session] in
            if session.isRunning {
                session.stopRunning()
            }
        }
    }

    private func configureSession() {
        session.beginConfiguration()
        defer { session.commitConfiguration() }
        session.sessionPreset = .high
        guard let device = AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: device),
              session.canAddInput(input) else { return }
        session.addInput(input)
        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else { return }
        session.addOutput(output)
        self.output = output
        output.setMetadataObjectsDelegate(self, queue: sessionQueue)
        // QR-only:其他码型不扫(契约面唯一形态)
        if output.availableMetadataObjectTypes.contains(.qr) {
            output.metadataObjectTypes = [.qr]
        }
    }

    /// 元数据回调(会话队列):只认 QR,原文投流(转主线程由 onReceive 承担)。
    nonisolated func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput metadataObjects: [AVMetadataObject],
        from connection: AVCaptureConnection,
    ) {
        guard let object = metadataObjects.first as? AVMetadataMachineReadableCodeObject,
              object.type == .qr,
              let text = object.stringValue, !text.isEmpty else { return }
        Task { @MainActor [weak self] in
            self?.payloadContinuation.yield(text)
        }
    }
}

/// 取景层宿主(AVCaptureVideoPreviewLayer 填满)。
private struct ScannerPreview: UIViewRepresentable {
    let scanner: ScannerCore

    final class PreviewHostView: UIView {
        override class var layerClass: AnyClass { AVCaptureVideoPreviewLayer.self }
        var previewLayer: AVCaptureVideoPreviewLayer { layer as! AVCaptureVideoPreviewLayer }
    }

    func makeUIView(context: Context) -> PreviewHostView {
        let view = PreviewHostView()
        view.previewLayer.session = scanner.session
        view.previewLayer.videoGravity = .resizeAspectFill
        return view
    }

    func updateUIView(_ uiView: PreviewHostView, context: Context) {}
}
