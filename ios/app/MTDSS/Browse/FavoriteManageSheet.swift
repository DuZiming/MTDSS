/// 快捷目录管理屏(V3-3 T17,spec §6「管理」;Windows FavManageDialog 同引擎
/// 语义的 iOS 消费面):sheet 持 id 每渲染经 store 查最新——他端改名/增删
/// 成员随 data_changed 秒级跟线,查无即 dismiss(他端删除快捷 = 解除链接,
/// sheet 自然收口)。
/// - 顶部 = 快捷名内联 TextField + 保存(busy/空名/未变更 禁;成功 toast 轻
///   反馈,失败 alert——store op 返 Bool 无明细面,plain 文案)。
/// - 成员列表 = 色点 + 设备名 + 路径 + 移除(免二次确认直接执行——ruling:
///   操作轻,可再添加;设备不在**已拉到**的快照 →「设备已移除」灰标,快照
///   拉取失败 → 名退化 id,不诬「已移除」——T16 混合页「失败保旧值」同象)。
/// - 底部 = 「删除快捷目录」destructive 钮(确认居中 alert,文案与根列表
///   同:只解除链接,不删除任何文件)。
/// - busy(store.busy)内容面全局禁点(store 级重入门的 UI 半边;「完成」
///   钮在门外,用户随时可走,sheet 下滑手势系统面不受 .disabled 影响)。

import MTDSSKit
import SwiftUI

struct FavoriteManageSheet: View {
    /// 快捷条目 id(持 id 不持值;每次渲染经 store 查最新)。
    let favId: String
    let repo: MtdssRepository
    /// 快捷 store(@ObservedObject 接 @Published 失效面:清单/名/成员/busy;
    /// sheet 自持观察,父页重渲与否不依赖)。
    @ObservedObject private var store: FavoritesStore

    /// 编辑框内容(开屏 = 当前名;提交 trim)。
    @State private var nameText = ""
    /// 最近一次已提交名(仅名**真变更**时回写编辑框——无关整单更新如成员
    /// 增删,不冲掉编辑中未提交文字)。
    @State private var committedName: String?
    /// 设备快照(成员名/色点 join 数据源;sheet 内现拉,DevicePickerSheet
    /// 同手法;spec §3.1「不入库,渲染时 join」)+ 是否拉到过。
    @State private var devices: [DirectoryDevice] = []
    @State private var devicesLoaded = false
    /// 成功轻反馈(重命名;T3 R3 动作反馈胶囊同语言)。
    @State private var toast: String?
    /// 操作失败文案(重命名/移除成员/删除;store op 返 Bool 无明细,plain)。
    @State private var opError: String?
    /// 删除确认(底部钮 → 居中 alert → 执行后自关)。
    @State private var confirmDelete = false

    @Environment(\.dismiss) private var dismiss

    init(favId: String, repo: MtdssRepository, store: FavoritesStore) {
        self.favId = favId
        self.repo = repo
        self.store = store
    }

    /// 最新快捷条目(store 单口;nil = 已被删——他端或本端)。
    private var favorite: Favorite? {
        store.favorites.first { $0.id == favId }
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            if let fav = favorite {
                content(fav)
            }
        }
        .background(Color(uiColor: .systemBackground))
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .task {
            // 开屏:名称种子 + 设备快照(互不阻塞顺序无涉,快照失败名退化);
            // 查无(推入窗内他端已删)即自关
            guard let fav = favorite else {
                dismiss()
                return
            }
            if committedName == nil {
                committedName = fav.name
                nameText = fav.name
            }
            await loadDevices()
        }
        .onChange(of: store.favorites) { _, favs in
            // 整单跟随:查无即自关(他端删除 = 解除链接,弹离即收,混合页
            // followFavoriteChange 同语义);名真变更回写编辑框(T15 concern②
            // @State 镜像驱动 body 重估同手法)
            guard let fav = favs.first(where: { $0.id == favId }) else {
                dismiss()
                return
            }
            if committedName != fav.name {
                committedName = fav.name
                nameText = fav.name
            }
        }
        // 删除确认(spec §3.1 解除链接文案;根列表同款逐字)
        .alert(
            "删除快捷目录",
            isPresented: $confirmDelete,
        ) {
            Button("删除", role: .destructive) {
                Task { await deleteFavorite() }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text(favorite.map { "只解除链接,不删除任何文件。删除 \($0.name)?" } ?? "")
        }
        // 操作失败(重命名/移除成员/删除;store op 返 Bool 无明细面,plain)
        .alert(
            "操作失败",
            isPresented: Binding(
                get: { opError != nil },
                set: { if !$0 { opError = nil } },
            ),
        ) {
            Button("好", role: .cancel) {}
        } message: {
            Text(opError ?? "")
        }
        .toast($toast)
    }

    // MARK: 内容面(重命名/成员在滚动区,删除钮沉底;busy 全局禁点)

    private func content(_ fav: Favorite) -> some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(spacing: 0) {
                    renameSection(fav)
                    memberSection(fav)
                }
            }
            deleteSection
        }
        // busy 全局禁点(store 在途编辑拒重入的 UI 半边;在途窗口短,「完成」
        // 在门外随时可走)
        .disabled(store.busy)
    }

    // MARK: 头部(标题居中 + 完成右;DevicePickerSheet 头部同语言)

    private var header: some View {
        ZStack {
            Text("管理快捷目录")
                .font(.system(size: 17, weight: .semibold))
                .lineLimit(1)
            HStack {
                Spacer()
                Button("完成") { dismiss() }
                    .font(.system(size: 17))
                    .foregroundStyle(Color.accentColor)
            }
        }
        .padding(.horizontal, 20)
        .padding(.top, 16)
        .padding(.bottom, 10)
    }

    // MARK: 快捷名(内联 TextField + 保存;busy/空名/未变更 禁)

    private func renameSection(_ fav: Favorite) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("快捷名")
                .font(.system(size: 13))
                .foregroundStyle(Color(uiColor: .systemGray))
            HStack(spacing: 10) {
                TextField("名称", text: $nameText)
                    .font(.system(size: 17))
                    .autocorrectionDisabled()
                    .padding(.horizontal, 10)
                    .frame(height: 40)
                    .background(Color(uiColor: .systemGray6), in: RoundedRectangle(cornerRadius: 11))
                Button("保存") {
                    Task { await saveName() }
                }
                .font(.system(size: 17))
                .foregroundStyle(Color.accentColor)
                // 空名禁提交 + 未变更禁提交(无差异回写无意义;busy 由外层
                // .disabled 承担)
                .disabled(trimmedName.isEmpty || trimmedName == fav.name)
            }
        }
        .padding(.horizontal, 20)
        .padding(.top, 6)
        .padding(.bottom, 12)
    }

    private var trimmedName: String {
        nameText.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// 重命名提交:成功 toast 轻反馈(编辑框经 onChange 整单跟随回写,提交值
    /// 即回写值,无视觉抖动);失败 plain alert,编辑保留可改可重试。
    private func saveName() async {
        let name = trimmedName
        guard !name.isEmpty else { return }
        guard await store.rename(repo: repo, id: favId, to: name) else {
            opError = "操作失败,请重试。"
            return
        }
        toast = "已重命名"
    }

    // MARK: 成员列表(色点 + 设备名 + 路径 + 移除;免二次确认)

    private func memberSection(_ fav: Favorite) -> some View {
        VStack(spacing: 0) {
            if fav.members.isEmpty {
                // 空成员(0 个合法,T14)引导(混合页空态同文案语言)
                Text("暂无成员,到设备浏览中添加目录")
                    .font(.system(size: 13))
                    .foregroundStyle(Color(uiColor: .systemGray))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 20)
                    .padding(.vertical, 14)
            } else {
                ForEach(Array(fav.members.enumerated()), id: \.offset) { _, member in
                    memberRow(member)
                }
            }
        }
    }

    /// 成员行:色点(色板 join,未知灰)+ 设备名(快照未拉到 → 退化 id 不
    /// 诬「已移除」;已拉到而无此设备 →「设备已移除」灰标)+ 路径原串小字灰
    /// +「移除」钮(直接执行,免二次确认——ruling:操作轻,可再添加)。
    private func memberRow(_ member: FavMember) -> some View {
        let known = devices.first { $0.deviceId == member.device }
        return FullBleedRow(height: 56) {
            HStack(spacing: 10) {
                Circle()
                    .fill(dotColor(member.device))
                    .frame(width: 6, height: 6)
                VStack(alignment: .leading, spacing: 2) {
                    if let known {
                        Text(known.name ?? known.deviceId)
                            .font(.system(size: 16))
                            .foregroundStyle(Color.primary)
                            .lineLimit(1)
                    } else if devicesLoaded {
                        Text("设备已移除")
                            .font(.system(size: 13))
                            .foregroundStyle(Color(uiColor: .systemGray))
                    } else {
                        Text(member.device)
                            .font(.system(size: 16))
                            .foregroundStyle(Color.primary)
                            .lineLimit(1)
                    }
                    Text(member.path)
                        .font(.system(size: 12.5))
                        .foregroundStyle(Color(uiColor: .systemGray))
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
                Button("移除") {
                    Task { await removeMember(member) }
                }
                .font(.system(size: 15))
                .foregroundStyle(Color.accentColor)
            }
            .padding(.horizontal, 20)
        }
    }

    /// 移除成员(解除链接):成功行随 store 更新消失,零额外反馈(轻操作);
    /// 失败 plain alert。
    private func removeMember(_ member: FavMember) async {
        guard await store.removeMember(repo: repo, id: favId, member: member) else {
            opError = "操作失败,请重试。"
            return
        }
    }

    // MARK: 删除快捷目录(底部 destructive;确认居中 alert,根列表同文案)

    private var deleteSection: some View {
        Button(role: .destructive) {
            confirmDelete = true
        } label: {
            Text("删除快捷目录")
                .font(.system(size: 17))
                .frame(maxWidth: .infinity, minHeight: 50)
        }
        .buttonStyle(PressableRowStyle())
        .padding(.horizontal, 20)
        .padding(.top, 6)
        .padding(.bottom, 12)
    }

    /// 删除快捷 = 解除链接(确认后):成功自关(根列表行随 store 更新消失;
    /// onChange 查无亦收,此处显式 dismiss 确定性收口);失败 plain alert。
    private func deleteFavorite() async {
        guard await store.delete(repo: repo, id: favId) else {
            opError = "操作失败,请重试。"
            return
        }
        dismiss()
    }

    // MARK: 设备快照 join(名/色点;失败保旧值——点灰、名退化,重开屏收口)

    private func loadDevices() async {
        let directory = try? await RpcGuard.withTimeout(seconds: 10) { [repo] in
            try await repo.devices()
        }
        if let directory {
            devices = directory.devices
            devicesLoaded = true
        }
    }

    /// 色点色(色 = ``FavoritesPalette`` 加入顺序位次映射,spec §13.4;
    /// 快照未及 = 灰,重开屏/未来刷新面收口)。
    private func dotColor(_ device: String) -> Color {
        let map = FavoritesPalette.colorMap(devices: devices)
        if let hex = map[device] {
            return Color(paletteHex: hex)
        }
        return Color(uiColor: .systemGray3)
    }
}
