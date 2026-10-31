/// 浏览页模板(spec §4.1,收件箱/信任目录/远端浏览共用):
/// 顶栏 → 搜索框(灰底圆角+放大镜,按名称过滤当前列表)→ 通栏行
/// (文件夹=左蓝›+蓝文件夹图标;文件=无箭头+问号文档图标;副文
/// 「日期 - N 项」/「日期 - 大小」)→「N 项」沉底。「···」菜单 =
/// 选择(占位,动作归 T6)+ 排序(名称/日期/大小/种类,单选勾)。
/// 浏览状态全在 ``BrowseViewModel``(MTDSSKit,宿主可测);本视图只装。

import MTDSSKit
import SwiftUI

struct BrowseView: View {
    @ObservedObject var vm: BrowseViewModel
    /// 推入页显示箭头;标签根页无箭头(仅三点)。
    var showsArrows: Bool
    /// 已在浏览根按返回 → 弹回标签根(常用目录)。
    var onPopToRoot: (() -> Void)? = nil

    var body: some View {
        VStack(spacing: 0) {
            BrowseNavBar(
                title: vm.title,
                showsArrows: showsArrows,
                canBack: vm.canGoBack,
                canForward: vm.canGoForward,
                onBack: {
                    if vm.canGoBack {
                        Task { await vm.goBack() }
                    } else {
                        onPopToRoot?()
                    }
                },
                onForward: { Task { await vm.goForward() } },
                menu: { overflowMenu },
            )
            searchBar
            listArea
        }
        .background(Color(uiColor: .systemBackground))
        .task {
            // 初次进页拉浏览根(错误态不自动重拉,重试按钮承担)
            if vm.entries.isEmpty && vm.errorText == nil && !vm.loading {
                await vm.loadRoot()
            }
        }
    }

    // MARK: 「···」菜单(选择占位 T6 + 排序单选勾)

    @ViewBuilder private var overflowMenu: some View {
        Button("选择") {
            // 占位:选择模式(顶栏全选/底栏操作)归 T6
        }
        Picker("排序", selection: $vm.sort) {
            ForEach(BrowseSort.allCases) { field in
                Text(field.label).tag(field)
            }
        }
    }

    // MARK: 搜索框(mockup .search:高 40,圆角 11,systemGray6 底)

    private var searchBar: some View {
        HStack(spacing: 7) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 14))
                .foregroundStyle(Color(uiColor: .systemGray))
            TextField("搜索", text: $vm.searchText)
                .font(.system(size: 15.5))
                .autocorrectionDisabled()
        }
        .padding(.horizontal, 10)
        .frame(height: 40)
        .background(Color(uiColor: .systemGray6), in: RoundedRectangle(cornerRadius: 11))
        .padding(.horizontal, 20)
        .padding(.top, 4)
        .padding(.bottom, 10)
    }

    // MARK: 列表区(错误态/加载态/列表+沉底尾注)

    @ViewBuilder private var listArea: some View {
        if let error = vm.errorText, vm.entries.isEmpty {
            ContentUnavailableView {
                Label("无法加载目录", systemImage: "exclamationmark.triangle")
            } description: {
                Text(error)
            } actions: {
                Button("重试") { Task { await vm.reload() } }
            }
        } else if vm.loading && vm.entries.isEmpty {
            VStack(spacing: 8) {
                Spacer()
                ProgressView()
                Spacer()
            }
        } else {
            SinkingFooterList {
                ForEach(vm.visibleEntries, id: \.path) { entry in
                    BrowseEntryRow(entry: entry) {
                        if entry.isDir {
                            Task { await vm.open(entry.path) }
                        }
                        // 文件行:点按动作(详情/选择)归 T6
                    }
                }
            } footer: {
                ItemCountFooter(count: vm.itemCount, note: vm.truncated ? "已截断" : nil)
            }
        }
    }
}

/// 浏览行(mockup .frow:高 58,行首内缩 20;文件夹左侧蓝 ›,文件保留
/// 箭头占位对齐名称列;细分割线自行首起)。
struct BrowseEntryRow: View {
    let entry: DirEntry
    var onOpen: () -> Void

    var body: some View {
        Button(action: onOpen) {
            FullBleedRow {
                HStack(spacing: 12) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(Color.accentColor)
                        .frame(width: 12)
                        .opacity(entry.isDir ? 1 : 0)
                    FileTypeIcon(isDir: entry.isDir)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(entry.name)
                            .font(.system(size: 17))
                            .foregroundStyle(Color.primary)
                            .lineLimit(1)
                        let sub = BrowseFormat.subtitle(entry)
                        if !sub.isEmpty {
                            Text(sub)
                                .font(.system(size: 12.5))
                                .foregroundStyle(Color(uiColor: .systemGray))
                                .lineLimit(1)
                        }
                    }
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 20)
            }
        }
        .buttonStyle(.plain)
    }
}

/// 排序字段的菜单文案(「···」→ 排序)。
extension BrowseSort {
    var label: String {
        switch self {
        case .name: "名称"
        case .date: "日期"
        case .size: "大小"
        case .kind: "种类"
        }
    }
}
