/// 照片网格页(R5c 方案 A;spec §4.2 选择骨架同款):
/// - 顶栏 = 相册名居中(返回弹回相册列表;「···」菜单「选择」);选择态
///   换 全选/「N 项」/确认(Ruling O:确认只退出),标签栏隐藏;
/// - 网格 = 3 列方格 LazyVGrid(缩略图 PHImageManager 降采样,utility 线程
///   同步请求 + 缓存);**格点按:选择态 = 切换选中;常态 = 无操作(v1 无
///   照片查看器,记录)**;长按任意格进选择(浏览页同款 0.4s);
/// - 选圈钉格左上角(R4 行首固定缘模式的网格版);**滑动删除不适用**
///   (照片非我方文件,无删除动作);
/// - 底部动作条 = 仅分享(square.and.arrow.up)→ 选设备弹层(4.3)→
///   vm.send(T11 管线:导出→暂存收件箱→SendService→终态 done 清暂存)。
/// 浏览状态全在 ``PhotoLibraryViewModel``(MTDSSKit,宿主可测);本视图只装。

import MTDSSKit
import SwiftUI

struct PhotoGridView: View {
    @ObservedObject var vm: PhotoLibraryViewModel
    let album: PhotoAlbum
    var onPopToRoot: () -> Void

    @Environment(AppModel.self) private var model
    @Environment(\.displayScale) private var displayScale
    /// 已捕获待发送的选择集(Ruling O:先捕获后退出;sheet(item:) 驱动)。
    @State private var pickedForSend: PickedPhotos?
    /// 动作发起反馈胶囊(R3 #3 同语言)。
    @State private var toast: String?

    private let thumbnailSide: CGFloat = 160

    var body: some View {
        VStack(spacing: 0) {
            if vm.isSelecting {
                SelectionNavBar(
                    count: vm.selection.count,
                    onSelectAll: { vm.selectAll() },
                    onConfirm: { vm.confirmSelection() },
                )
            } else {
                BrowseNavBar(
                    title: album.name,
                    showsArrows: true,
                    canBack: true,
                    canForward: false,
                    onBack: { onPopToRoot() },
                    onForward: nil,
                    menu: { overflowMenu },
                )
            }
            gridArea
        }
        .background(Color(uiColor: .systemBackground))
        .toolbar(.hidden, for: .navigationBar)
        // 选择模式隐藏标签栏(spec §4.2;R6 #2 同修:条件挂载——本页推在
        // 首页标签栈内,根页声明竞争面与浏览推入页同形)
        .tabBarHiddenWhenSelecting(vm.isSelecting)
        .toast($toast)
        .safeAreaInset(edge: .bottom) {
            if vm.isSelecting {
                // 仅分享(照片无删除/下载/转发面)
                SelectionActionBar(actions: [.share]) { _ in
                    shareSelection()
                }
            }
        }
        .sheet(item: $pickedForSend) { picked in
            // repo 门面:图库入口本身要求引擎就绪(首页根页已收),防御占位
            if let repo = model.repository {
                DevicePickerSheet(
                    title: "发送 \(picked.items.count) 项到…",
                    repo: repo,
                    selfDeviceId: model.selfDeviceId,
                ) { device in
                    startSend(picked.items, to: device)
                }
            }
        }
        .alert(
            "操作失败",
            isPresented: Binding(
                get: { vm.lastActionError != nil },
                set: { if !$0 { vm.lastActionError = nil } },
            ),
        ) {
            Button("好", role: .cancel) {}
        } message: {
            Text(vm.lastActionError ?? "")
        }
        .task {
            // 进页拉网格(错误态不自动重拉,重试按钮承担)
            if vm.currentAlbum != album {
                await vm.open(album: album)
            }
        }
    }

    // MARK: 「···」菜单(选择;照片无排序/刷新面)

    @ViewBuilder private var overflowMenu: some View {
        Button("选择") {
            vm.enterSelection()
        }
    }

    // MARK: 选择流(Ruling O:捕获 → 退出 → 弹层;动作仅自底栏)

    private func shareSelection() {
        let picked = vm.selectedAssets
        guard !picked.isEmpty else { return }
        vm.exitSelection()
        pickedForSend = PickedPhotos(items: picked)
    }

    /// 发送到设备(弹层点选后):vm.send 全链(导出→暂存→发送→终态清理)。
    /// 受理成功 toast(R3 #3);失败走 lastActionError 提示。
    private func startSend(_ assets: [PhotoAsset], to device: DirectoryDevice) {
        guard !vm.sending else { return }
        Task {
            let ok = await vm.send(assets, to: device)
            if ok {
                toast = "已开始 \(assets.count) 项 · 进度见传输列表"
            }
        }
    }

    // MARK: 网格区(加载/空/错误态 + 3 列方格)

    @ViewBuilder private var gridArea: some View {
        if let error = vm.assetsError, vm.assets.isEmpty {
            ContentUnavailableView {
                Label("无法加载照片", systemImage: "exclamationmark.triangle")
            } description: {
                Text(error)
            } actions: {
                Button("重试") { Task { await vm.reloadAssets() } }
            }
        } else if vm.assetsLoading && vm.assets.isEmpty {
            VStack(spacing: 8) {
                Spacer()
                ProgressView()
                Spacer()
            }
        } else if vm.assets.isEmpty {
            ContentUnavailableView(
                "没有照片",
                systemImage: "photo",
                description: Text("该相册暂无照片"),
            )
        } else {
            ScrollView {
                LazyVGrid(
                    columns: Array(repeating: GridItem(.flexible(), spacing: 2), count: 3),
                    spacing: 2,
                ) {
                    ForEach(vm.assets) { asset in
                        PhotoGridCell(
                            asset: asset,
                            thumbnailSide: thumbnailSide,
                            displayScale: displayScale,
                            service: PHPhotoLibraryService.shared,
                            selecting: vm.isSelecting,
                            selected: vm.selection.contains(asset.id),
                            onToggle: {
                                // 选择态点按 = 切换选中;常态无操作(v1 无查看器)
                                if vm.isSelecting {
                                    vm.toggle(asset.id)
                                }
                            },
                        )
                        // 长按任意格进选择(浏览页同款 0.4s)
                        .simultaneousGesture(
                            LongPressGesture(minimumDuration: 0.4).onEnded { _ in
                                if !vm.isSelecting { vm.enterSelection() }
                            },
                        )
                    }
                }
                .padding(.horizontal, 2)
                .padding(.top, 2)
                ItemCountFooter(count: vm.assets.count)
            }
            .animation(.easeInOut(duration: 0.2), value: vm.isSelecting)
        }
    }
}

/// 已捕获待发送的照片选择集(photos 版 ``PickedSelection``)。
struct PickedPhotos: Identifiable {
    let id = UUID()
    let items: [PhotoAsset]
}

/// 网格格(3 列方格):缩略图 + 选圈(选择态钉左上角,R4 固定缘模式网格
/// 版;选中 = 蓝底白勾);点按 = onToggle(选择态切换/常态无操作)。
struct PhotoGridCell: View {
    let asset: PhotoAsset
    let thumbnailSide: CGFloat
    let displayScale: CGFloat
    let service: PHPhotoLibraryService
    let selecting: Bool
    let selected: Bool
    let onToggle: () -> Void

    @State private var image: UIImage?

    var body: some View {
        Button(action: onToggle) {
            // 方格基底(Color.clear+aspectRatio 定死 1:1)+ overlay 装内容 +
            // .clipped 裁溢出——scaledToFill 的缩略图越界不得挤压邻格
            // (初版 ZStack 直排,缩略图按自身尺寸外溢压叠,真机形态 bug)
            Color.clear
                .aspectRatio(1, contentMode: .fit)
                .overlay {
                    ZStack(alignment: .topLeading) {
                        Rectangle()
                            .fill(Color(uiColor: .systemGray6))
                        if let image {
                            Image(uiImage: image)
                                .resizable()
                                .scaledToFill()
                        }
                        if selecting {
                            selectionCircle
                                .padding(6)
                                .transition(.opacity.combined(with: .scale(scale: 0.8)))
                        }
                    }
                }
                .clipped()
                .contentShape(Rectangle())
                .opacity(selected ? 0.85 : 1)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(Text((asset.filename ?? "照片") + (selected ? ",已选" : "")))
        .task(id: asset.id) {
            image = await service.thumbnail(
                for: asset.id, side: thumbnailSide, scale: displayScale)
        }
    }

    /// 选圈(24pt,1.5 灰描边;选中 = 蓝底白勾——BrowseEntryRow 同款)。
    private var selectionCircle: some View {
        ZStack {
            if selected {
                Circle().fill(Color.accentColor)
                Image(systemName: "checkmark")
                    .font(.system(size: 12, weight: .bold))
                    .foregroundStyle(.white)
            } else {
                Color.clear
                Circle()
                    .strokeBorder(Color(uiColor: .systemGray4), lineWidth: 1.5)
                    .background(Color.white, in: Circle())
            }
        }
        .frame(width: 24, height: 24)
    }
}
