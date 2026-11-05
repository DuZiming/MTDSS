/// 「添加到常用目录」选择弹层(V3-3 T18,spec §5.3 弹层面/§6 添加成员入口;
/// Windows FavPickerDialog 同引擎语义的 iOS 消费面):远端浏览档目录行左滑
/// 「常用」(单条)与选择底栏「常用」(纯目录多选)统一入口,由设备页
/// 推入浏览页(``RemoteBrowseDestination``)挂载。
/// - 清单 = 常用条目(名 + N 个成员,创建序);点选 → 行内预去重(Windows
///   T11 同位预检:全重复直接「该目录已在其中」收口,不发回写)→
///   ``FavoritesStore/addMembers`` → 实际新增>0 = toast「已加入 {名}」;
///   =0(预去重后仍零新增 = put 失败/只读守卫,store 吞错不区分)= 失败
///   alert 留驻可重试。成功/已在其中 = toast 短暂呈现后收弹层(胶囊随弹层
///   拆建,即刻收则反馈不可见——0.9s 读面,注释钉);
/// - 空清单(非 readOnly)=「还没有常用目录」+「新建常用目录」行(alert
///   +TextField,T17 首页同款,空名禁提交)→ create → **认领取尾部 id**
///   (T14 创建序不变量:create 恒尾部追加且 put 成功才落 @Published,
///   create 返 true 后 favorites.last 必为本端新条目;控制者裁定钉此依赖,
///   不用 prevIds 差集)→ addMembers → toast → 收;
/// - readOnly(v>1 前向守卫)= 只读提示行,清单照渲不可加(防翻转窗内误加;
///   store 本体绝不回写,T14);
/// - busy 禁点(store 编辑重入门的 UI 半边;storeTick 经 onReceive 观测 +
///   working 本地双击闸双保险——双击快于 busy 观测窗时本地闸先收口)。

import MTDSSKit
import SwiftUI

/// 「加入常用」待选载荷(V3-3 T18):[DirEntry] 无 Identifiable,sheet(item:)
/// 薄包装;sourceDevice = FavMember.device(浏览目标设备 id,spec §3.1
/// 成员只存 device_id+path)。
struct FavoritePickRequest: Identifiable {
    let id = UUID()
    let entries: [DirEntry]
    let sourceDevice: String
}

struct FavoritePickerSheet: View {
    /// 待加入的目录集(swipe 单条 = 1 枚;底栏多选 = 纯目录集)。
    let entries: [DirEntry]
    /// 成员 device = 浏览目标设备 id(FavMember.device)。
    let sourceDevice: String
    /// 收弹层回调(父 sheet(item:) 状态清 nil;下滑关闭系统面自收)。
    let onDismiss: () -> Void

    @Environment(AppModel.self) private var model
    /// store @Published 失效面重渲(Environment(AppModel.self) 只跟踪
    /// AppModel 自有属性,子 store 的 Combine 发布不入观察——T16 onReceive
    /// 同课):busy/readOnly/清单任一变化 bump tick 驱动 body 重估。
    @State private var storeTick = 0
    /// 本弹层 addMembers/create 在途(双击防线;busy 观测窗外的本地先手闸)。
    @State private var working = false
    /// 新建常用目录对话框(空清单入口;T17 首页同款)。
    @State private var showCreate = false
    @State private var newName = ""
    /// 操作失败文案(put 失败/只读守卫;store op 返 Bool 无明细面,plain)。
    @State private var opError: String?
    /// 结果反馈胶囊(「已加入 {名}」/「该目录已在其中」)。
    @State private var toast: String?

    private var store: FavoritesStore { model.favorites }

    var body: some View {
        VStack(spacing: 0) {
            header
            listArea
        }
        .background(Color(uiColor: .systemBackground))
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .onReceive(store.$busy) { _ in storeTick += 1 }
        .onReceive(store.$readOnly) { _ in storeTick += 1 }
        .onReceive(store.$favorites) { _ in storeTick += 1 }
        // 新建常用目录(命名对话框;空名禁提交——钮 disabled,T17 首页
        // alert+TextField 同款逐字)
        .alert("新建常用目录", isPresented: $showCreate) {
            TextField("名称", text: $newName)
            Button("创建") {
                let name = newName.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !name.isEmpty else { return }
                Task { await createAndAdd(name) }
            }
            .disabled(newName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            Button("取消", role: .cancel) {}
        } message: {
            Text("创建后可把设备里的目录添加进来")
        }
        // 操作失败(addMembers put 失败/只读守卫;store op 返 Int 无明细面,
        // plain 文案 = T17 同口径)
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

    // MARK: 头部(标题居中 + 完成;FavoriteManageSheet 头部同语言)

    private var header: some View {
        ZStack {
            Text("添加到常用目录")
                .font(.system(size: 17, weight: .semibold))
                .lineLimit(1)
            HStack {
                Spacer()
                Button("完成") { onDismiss() }
                    .font(.system(size: 17))
                    .foregroundStyle(Color.accentColor)
            }
        }
        .padding(.horizontal, 20)
        .padding(.top, 16)
        .padding(.bottom, 10)
    }

    // MARK: 清单(常用条目行;readOnly 提示 + 空态新建入口)

    @ViewBuilder private var listArea: some View {
        ScrollView {
            VStack(spacing: 0) {
                if store.readOnly {
                    hintRow("常用目录当前只读,暂不能添加")
                }
                if store.favorites.isEmpty {
                    if !store.readOnly {
                        // 空清单(非 readOnly):空态文案 + 新建入口(Windows
                        // picker 空态「还没有常用目录」同文案;新建是唯一出路)
                        hintRow("还没有常用目录")
                        newRow
                    }
                } else {
                    ForEach(store.favorites) { fav in
                        favoriteRow(fav)
                    }
                }
            }
        }
        // busy 禁点(store 在途编辑拒重入的 UI 半边;「完成」在门外随时可走,
        // sheet 下滑手势系统面不受 .disabled 影响——FavoriteManageSheet 同形)
        .disabled(store.busy || working)
    }

    /// 提示行(muted 不可点;T17 空清单 hint 同形)。
    private func hintRow(_ text: String) -> some View {
        FullBleedRow {
            Text(text)
                .font(.system(size: 14))
                .foregroundStyle(Color(uiColor: .systemGray))
                .padding(.horizontal, 20)
        }
        .accessibilityElement(children: .combine)
    }

    /// 空清单新建入口(整行 accent;Windows picker「新建常用目录」同位)。
    private var newRow: some View {
        Button {
            newName = ""
            showCreate = true
        } label: {
            HStack(spacing: 8) {
                Image(systemName: "plus.circle.fill")
                Text("新建常用目录")
            }
            .font(.system(size: 17))
            .frame(maxWidth: .infinity, minHeight: 50)
        }
        .buttonStyle(PressableRowStyle())
        .accessibilityLabel(Text("新建常用目录"))
    }

    /// 常用条目行:RootFolderIcon(首页常用行同视觉)+ 名 + 尾部「N 个成员」
    /// 灰注(Windows FavPickerDialog meta 同文案);点选即加。
    private func favoriteRow(_ fav: Favorite) -> some View {
        Button {
            Task { await addTo(fav) }
        } label: {
            FullBleedRow(height: 56) {
                HStack(spacing: 12) {
                    RootFolderIcon()
                    Text(fav.name)
                        .font(.system(size: 17))
                        .foregroundStyle(Color.primary)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                    Text("\(fav.members.count) 个成员")
                        .font(.system(size: 12.5))
                        .foregroundStyle(Color(uiColor: .systemGray))
                }
                .padding(.horizontal, 20)
            }
            // 整行触区(R3 同款:透明区可点)
            .contentShape(Rectangle())
        }
        .buttonStyle(PressableRowStyle())
        .accessibilityLabel(Text("\(fav.name),\(fav.members.count) 个成员"))
    }

    // MARK: 加入流(点选已有 / 空态新建即加;Windows T11 同源语义)

    /// 点选加入:目标缺失(他端删除窗)静默收(清单已变,Windows 同象);
    /// 预去重(全重复「该目录已在其中」收口,不发回写)→ addMembers →
    /// 实际新增>0 = toast「已加入 {名}」;=0 = 失败 alert 留驻可重试。
    private func addTo(_ fav: Favorite) async {
        guard let repo = model.repository else { return }
        guard !working, !store.busy else { return }
        guard let target = store.favorites.first(where: { $0.id == fav.id }) else {
            onDismiss() // 他端删除随 data_changed 跟线,弹层照收
            return
        }
        var seen = Set(target.members.map(memberKey))
        let fresh = entries.filter { seen.insert(memberKey($0)).inserted }
        if fresh.isEmpty {
            toast = "该目录已在其中"
            await pauseBeforeDismiss()
            onDismiss()
            return
        }
        working = true
        defer { working = false }
        let added = await store.addMembers(
            repo: repo, id: fav.id,
            members: fresh.map { FavMember(device: sourceDevice, path: $0.path) })
        if added > 0 {
            toast = "已加入 \(target.name)"
            await pauseBeforeDismiss()
            onDismiss()
        } else {
            opError = "操作失败,请重试。"
        }
    }

    /// 新建即加(Windows T11 Final ruling 同源:新建落库后认领新条目直加)。
    /// **认领取尾部 id**——依赖 T14 创建序不变量:create 恒尾部追加且 put
    /// 成功才落 @Published,create 返 true 后 favorites.last 必为本端新条目
    /// (控制者裁定钉此依赖,不用 prevIds 差集)。新建成功后的 addMembers
    /// 失败不回滚新建(常用已建成,可重开弹层再加,Windows 同口径)。
    private func createAndAdd(_ name: String) async {
        guard let repo = model.repository else { return }
        guard !working, !store.busy else { return }
        working = true
        defer { working = false }
        guard await store.create(repo: repo, name: name) else {
            opError = "操作失败,请重试。"
            return
        }
        guard let fresh = store.favorites.last else { // T14 创建序不变量(头注)
            opError = "操作失败,请重试。"
            return
        }
        let added = await store.addMembers(
            repo: repo, id: fresh.id,
            members: entries.map { FavMember(device: sourceDevice, path: $0.path) })
        if added > 0 {
            toast = "已加入 \(fresh.name)"
            await pauseBeforeDismiss()
            onDismiss()
        } else {
            // 新建条目空成员起步,零新增只剩写入失败(put 失败/只读守卫)
            // ——失败 alert 留驻,常用条目已建成可重开再加
            opError = "操作失败,请重试。"
        }
    }

    /// 预去重键(NUL 分隔防拼接歧义;``FavoritesStore`` 私有 memberKey 的
    /// App 层复刻——预检纯 App 层判定,store 侧 addMembers 仍按同键终审去重)。
    private func memberKey(_ entry: DirEntry) -> String {
        sourceDevice + "\u{0000}" + entry.path
    }

    private func memberKey(_ m: FavMember) -> String {
        m.device + "\u{0000}" + m.path
    }

    /// 收弹层前的 toast 读面停(胶囊 0.2s 淡入,即刻收则反馈不可见;
    /// 0.9s 短读面后父层收弹层)。
    private func pauseBeforeDismiss() async {
        try? await Task.sleep(nanoseconds: 900_000_000)
    }
}
