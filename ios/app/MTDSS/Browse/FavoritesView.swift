/// 第一标签「快捷目录」根页(2026-11-01 Ruling N:唯一展示项 = 收件箱。
/// 授权集 = 收件箱(默认)+ 信任目录(白名单)——信任目录只进引擎授权集
/// (engine-roots.conf)与设置页管理列表,不投影首页)。数据源 =
/// AuthorizedRoot.roots(recvDir:)。根页无返回/前进箭头,仅三点
/// (spec §4.1);点根条目推入浏览页(4.1 模板)。
/// **根行行内展开**(R4 真机轮 #1):箭头 = 行内展开子级(缩进逐级、懒
/// 加载、同 VM 机制),行点按 = 进整页浏览;子行点目录 = 推入该子目录的
/// 整页浏览(带 initialPath)。
/// **选择模式**(R5b #1,spec §4.2 同一骨架):「···」菜单「选择」或
/// 长按任意子行(depth≥1)进入——顶栏换 全选/「N 项」/确认(Ruling O
/// 确认只退出),子行首选圈,**根行(收件箱)不可选(无圈)**,标签栏
/// 隐藏,底部动作条 = 分享/删除;确认/动作语义与浏览页同一 VM 面
/// (``BrowseViewModel`` isSelecting/selectedEntries 复用,零重复)。
/// **快捷行树形=成员内容合并**(V3-3 返工二 §14.1,推翻 §13.8 成员目录
/// 节点形态):快捷行箭头 = 行内展开**成员内容并集**——各**在线**成员目录
/// 列表的并集(文件与文件夹混排、目录恒前+名称升序,Kit 纯投影
/// ``FavoritesTree``,混合视图 ``MixedViewModel`` 扇出纪律同源),行带来源
/// 终端色点;目录行箭头 = 懒加载该目录在其所属设备上的子级(递归,文件
/// 各级可见);点目录 = 推成员目录整页浏览(.member 路由);文件行仅展示
/// 不响应(无点按无滑面);离线/失败成员条目缺席,定局后尾部「N 个成员
/// 离线」注记。点按/导航全部经 ``navigate``(NavPathGate §13.7 闸)。
/// 左滑 = [管理, 删除快捷](管理 = ``FavoriteManageSheet``;删除快捷 =
/// 确认后 favorites.delete,只解除链接,仅快捷行自身)。「···」菜单增
/// 「新建快捷目录」(alert+TextField,空名禁提交);清单空 → hint 行引导;
/// readOnly(v>1 前向守卫)时滑删/新建全禁、清单照渲(T14 store 本体绝不
/// 回写)。

import MTDSSKit
import SwiftUI

struct FavoritesView: View {
    @Environment(AppModel.self) private var model
    /// 值路由(R4 #1 起):根页推入 = 整页浏览根(.root)或子目录(.sub,
    /// 子行点按进整页)。全部写入经 ``navigate``(NavPathGate §13.7 重入闸)。
    @State private var navPath: [FavoritesRoute] = []
    /// 导航重入闸(§13.7:转场窗内值路径再变更 = SwiftUI boundPathChange
    /// 断言崩溃;根列表行连点/秒退再点即触达)。
    @State private var navGate = NavPathGate<FavoritesRoute>()

    /// 值路径写入唯一入口(§13.7 闸;推入 = [目标],弹回 = [])。
    private func navigate(_ target: [FavoritesRoute]) {
        navGate.write(target, to: $navPath)
    }

    /// 首页根列表(Ruling N:收件箱单枚;信任目录不入首页)。
    private var roots: [AuthorizedRoot] {
        AuthorizedRoot.roots(recvDir: model.recvDir)
    }

    var body: some View {
        NavigationStack(path: $navPath) {
            // ZStack 定壳:navigationDestination 挂在**恒存容器**上——直挂
            // rootPage 时,spinner→列表分支互换会重挂 destination 修饰符并
            // **重置导航栈**(R5c probe 钉:推入后 status 投影到达即被弹回;
            // 真用户在首屏加载窗内点按同落此坑)
            ZStack {
                rootPage
            }
            .navigationDestination(for: FavoritesRoute.self) { route in
                destination(route)
            }
        }
        #if DEBUG
        .onChange(of: navPath) { _, newValue in
            print("[R5C-E2E] FAV navPath -> \(newValue.count) 项: \(newValue.map { "\($0)" }.joined(separator: ","))")
        }
        .onAppear { print("[R5C-E2E] FAV appear") }
        #endif
        .task {
            await model.refreshStatus()
            await pushInboxIfDebugHooked()
            #if DEBUG
            await rapidNavIfHooked()
            #endif
        }
    }

    // MARK: 根页

    @ViewBuilder private var rootPage: some View {
        if let bootError = model.bootError {
            // 引擎未就绪:占位页(无选择面;菜单空——boot 前无可操作项)
            VStack(spacing: 0) {
                BrowseNavBar(title: "快捷目录", showsArrows: false) { EmptyView() }
                ContentUnavailableView {
                    Label("引擎未就绪", systemImage: "cpu")
                } description: {
                    Text(bootError)
                }
            }
            .background(Color(uiColor: .systemBackground))
            .toolbar(.hidden, for: .navigationBar)
        } else if roots.isEmpty || model.repository == nil {
            // status 未投影 / 引擎重启窗口:转圈兜底(信任目录变更面)
            VStack(spacing: 0) {
                BrowseNavBar(title: "快捷目录", showsArrows: false) { EmptyView() }
                VStack(spacing: 8) {
                    Spacer()
                    ProgressView()
                    Spacer()
                }
            }
            .background(Color(uiColor: .systemBackground))
            .toolbar(.hidden, for: .navigationBar)
        } else if let repo = model.repository {
            // 首页列表(R4 #1:根行可展开;R5b #1:选择模式。VM =
            // BrowseViewModel 复用,顶栏/选择顶栏随 VM 态在列表组件内收口)
            FavoritesRootList(
                repo: repo,
                selfDeviceId: model.selfDeviceId,
                inbox: roots[0],
                favorites: model.favorites,
                showHidden: model.showHidden,
                onOpenRoot: { navigate([.root($0)]) },
                onOpenPath: { navigate([.sub(roots[0], path: $0)]) },
                onOpenFavorite: { navigate([.fav($0)]) },
                onOpenMember: { navigate([.member($0, $1, path: $2)]) },
            )
        }
    }

    // MARK: 推入页(浏览模板)

    @ViewBuilder private func destination(_ route: FavoritesRoute) -> some View {
        switch route {
        case .fav(let fav):
            // V3-3 快捷混合页(T16;T17 根列表行触发推入)
            if let repo = model.repository {
                FavoritesMixedView(
                    fav: fav,
                    repo: repo,
                    selfDeviceId: model.selfDeviceId,
                    onPopToRoot: { navigate([]) },
                )
            } else {
                bootPlaceholder(title: fav.name, hint: "引擎启动完成后即可查看「\(fav.name)」")
            }
        case .member(_, let member, let path):
            // V3-3 返工 §13.8:快捷成员目录整页浏览(浏览根 = 目标路径,
            // 标题 = 目录显示名不携设备名;远端档动作集随 isRemote,device =
            // 成员真实 id 本机短路同口)
            if let repo = model.repository {
                let tail = BrowseFormat.displayName(ofPath: path)
                BrowseDestination(
                    root: AuthorizedRoot(name: tail, path: path, isInbox: false),
                    repo: repo,
                    deviceId: member.device,
                    isRemote: member.device != model.selfDeviceId,
                    onPopToRoot: { navigate([]) },
                )
            } else {
                let tail = BrowseFormat.displayName(ofPath: path)
                bootPlaceholder(title: tail, hint: "引擎启动完成后即可浏览\(tail)")
            }
        case .root, .sub:
            if let repo = model.repository {
                // 本地目录浏览不设登录门(T13b 修#4:daemon 本机短路不要求
                // 会话;未登录 device 参数走保留别名 "self")。
                BrowseDestination(
                    root: route.root,
                    repo: repo,
                    deviceId: BrowseViewModel.localBrowseDeviceId(selfDeviceId: model.selfDeviceId),
                    initialPath: route.initialPath,
                    onPopToRoot: { navigate([]) },
                )
            } else {
                bootPlaceholder(title: route.root.name, hint: "引擎启动完成后即可浏览\(route.root.name)")
            }
        }
    }

    /// 引擎未就绪推入页占位(repository 缺;bootError 横幅由 AppTabView
    /// 顶层承担):给返回路的占位。
    private func bootPlaceholder(title: String, hint: String) -> some View {
        VStack(spacing: 0) {
            BrowseNavBar(
                title: title,
                showsArrows: true,
                canBack: false,
                canForward: false,
                onBack: { navigate([]) },
                menu: { EmptyView() },
            )
            ContentUnavailableView(
                "引擎未就绪",
                systemImage: "cpu",
                description: Text(hint),
            )
        }
        .background(Color(uiColor: .systemBackground))
        .toolbar(.hidden, for: .navigationBar)
    }

    // MARK: DEBUG 冒烟钩子(模拟器无触点驱动;截图用,真机/产线不编译)

    private func pushInboxIfDebugHooked() async {
        #if DEBUG
        guard ProcessInfo.processInfo.environment["MTDSS_DEBUG_AUTOPUSH"] == "inbox" else { return }
        // 等 status 投影就绪(recv_dir 随引擎 boot/status 异步到达;5s 上界)
        for _ in 0..<50 {
            if let inbox = roots.first(where: \.isInbox) {
                if navPath.isEmpty { navigate([.root(inbox)]) }
                return
            }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        #endif
    }

    #if DEBUG
    /// MTDSS_DEBUG_FAVOPEN=rapid:导航重入闸回归驱动(§13.7 崩溃修复回归
    /// 锚,FavTapCrashUITests 消费;模拟器无触点节奏控制的先例面 =
    /// MTDSS_DEBUG_AUTOPUSH 同类 DEBUG 钩子,真机/产线不编译)。节奏化
    /// pop/push 轮转(200ms 一拍,连发 10 轮)——修复前此节奏恒触
    /// SwiftUI NavigationStack boundPathChange 断言崩溃(sim .ips 在案),
    /// 修复后经闸错峰全程存活。推入页 = 收件箱根(.root 与 .fav 同为值
    /// 路由,转场机器同一面,零注入依赖)。
    ///
    /// **轮转可观测面**(R9 修复轮,防空转通过):收尾把完成轮数写
    /// `{MTDSS_HOME}/favopen-rapid-marker`(文件内容 = 轮数;提前退出 =
    /// "0")。runner 进程与 app 同机同用户,scheme 经 UITEST_MTDSS_HOME 注
    /// 入同一路径——UITest 断言标记在场且 ≥10,守卫静默退出/轮转未跑即
    /// 测试红(锚不再可无声退化)。
    private func rapidNavIfHooked() async {
        guard ProcessInfo.processInfo.environment["MTDSS_DEBUG_FAVOPEN"] == "rapid" else { return }
        // 等收件箱根投影(status.refresh 后 recv_dir 就绪;5s 上界)
        var inbox: AuthorizedRoot?
        for _ in 0..<50 {
            if let found = roots.first(where: \.isInbox) {
                inbox = found
                break
            }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        guard let inbox else {
            print("[FAVOPEN-RAPID] 收件箱根未投影(引擎未就绪?),钩子退出")
            writeRapidMarker(0)
            return
        }
        navigate([.root(inbox)])
        try? await Task.sleep(nanoseconds: 1_000_000_000)
        var rounds = 0
        for i in 0..<10 {
            navigate([])
            print("[FAVOPEN-RAPID] pop #\(i)")
            try? await Task.sleep(nanoseconds: 200_000_000)
            navigate([.root(inbox)])
            print("[FAVOPEN-RAPID] push #\(i)")
            try? await Task.sleep(nanoseconds: 200_000_000)
            rounds += 1
        }
        writeRapidMarker(rounds)
    }

    /// 轮转计数落盘(可观测面;写失败静默——标记缺席即 UITest 红,同收口)。
    private func writeRapidMarker(_ rounds: Int) {
        guard let home = ProcessInfo.processInfo.environment["MTDSS_HOME"], !home.isEmpty else { return }
        let url = URL(fileURLWithPath: home).appendingPathComponent("favopen-rapid-marker")
        try? Data("\(rounds)\n".utf8).write(to: url)
    }
    #endif
}

/// 首页导航目标:根页推入浏览根、子行点按直入子目录整页(R4 #1)。
/// **子目录档单目的直入**(2026-11-02 R4 用户裁定回炉):子页 VM 以子
/// 目录为浏览根(进页 canGoBack=false → 顶栏返回/边缘右滑 = onPopToRoot
/// 弹回首页,无中间收件箱页);页内再进 = 正常页内历史。
enum FavoritesRoute: Hashable {
    case root(AuthorizedRoot)
    case sub(AuthorizedRoot, path: String)
    /// V3-3 快捷混合页(T16:路由 + destination 分支;T17 根列表行触发
    /// 推入。Favorite Hashable,T14 面)。
    case fav(Favorite)
    /// V3-3 返工 §13.8:快捷成员目录直入浏览页(§14.1 起触发面 = 快捷树
    /// 目录行——合并行/懒加载子目录行点按;浏览根 = 目标路径;标题 = 目录
    /// 显示名、设备隐含——与 §13.6 Windows 面包屑「不显示设备名与原始路径」
    /// 同口径;Favorite 仅路由上下文,页面数据面 = device+path,device 取
    /// 目标真实 id:引擎 is_self 认真实 id[本机成员短路同口,T13b 修#4])。
    case member(Favorite, FavMember, path: String)

    var root: AuthorizedRoot {
        switch self {
        case .root(let root): root
        case .sub(let root, _): root
        // 混合页/成员档不经本投影(destination 按 case 分派;值恒不被消费)
        case .fav, .member: AuthorizedRoot(name: "", path: "", isInbox: false)
        }
    }

    /// 子目录整页直入路径(.root = nil,开页走 loadRoot;.fav 无初始路径;
    /// .member 亦无——成员档浏览根即目标路径,直传 AuthorizedRoot)。
    var initialPath: String? {
        switch self {
        case .root, .fav, .member: nil
        case .sub(_, let path): path
        }
    }
}

/// 浏览推入页:每根一枚 VM(StateObject 随页生命周期);showHidden 以
/// 设置开关为源(App 侧,T10),开页取值 + 开关变化活体跟随。
/// initialPath(R4 #1 用户裁定回炉)= 子目录直入单目的:**VM 以子目录为
/// 浏览根**(history = [子目录],进页 canGoBack=false → 顶栏返回/边缘
/// 右滑 = onPopToRoot 弹回首页,无中间收件箱页;标题 = 子目录名,与原
/// 深级口径一致);nil = 常规开页(浏览根 = root.path)。
/// V3-3 T16 起混合页钻取复用(去 private):+isRemote/+deviceName 两缺省
/// 参(远端档动作集与「设备名 | 目录名」标题);既有 .root/.sub 调用
/// 零变化。
struct BrowseDestination: View {
    let root: AuthorizedRoot
    let repo: MtdssRepository
    let deviceId: String
    var initialPath: String? = nil
    /// 远端档(T7 动作集:下载/转发+删除门;缺省 false = 本地档既有语义)。
    var isRemote: Bool = false
    /// 对端设备名(R4 #5 标题「设备名 | 目录名」;缺省 nil = 单段标题)。
    var deviceName: String? = nil
    let onPopToRoot: () -> Void

    @Environment(AppModel.self) private var model
    @StateObject private var vm: BrowseViewModel

    init(
        root: AuthorizedRoot,
        repo: MtdssRepository,
        deviceId: String,
        initialPath: String? = nil,
        isRemote: Bool = false,
        deviceName: String? = nil,
        onPopToRoot: @escaping () -> Void,
    ) {
        self.root = root
        self.repo = repo
        self.deviceId = deviceId
        self.initialPath = initialPath
        self.isRemote = isRemote
        self.deviceName = deviceName
        self.onPopToRoot = onPopToRoot
        _vm = StateObject(wrappedValue: BrowseViewModel(
            repo: repo,
            deviceId: deviceId,
            rootPath: initialPath ?? root.path,
            rootTitle: initialPath.map { BrowseFormat.displayName(ofPath: $0) } ?? root.name,
            deviceName: deviceName,
        ))
    }

    var body: some View {
        BrowseView(
            vm: vm,
            showsArrows: true,
            onPopToRoot: onPopToRoot,
            sendService: SendService(repo: repo),
            isRemote: isRemote,
        )
        .toolbar(.hidden, for: .navigationBar)
        .task { vm.showHidden = model.showHidden }
        .onChange(of: model.showHidden) { _, hidden in
            vm.showHidden = hidden
        }
    }
}

/// 首页根列表(R4 真机轮 #1:根行同浏览行的行内展开语义——箭头 = 展开
/// 子级行内呈现,行点按 = 进整页浏览;R5b #1:选择模式,顶栏/底栏随 VM
/// 态收口在本组件——菜单「选择」与行长按两入口同落 ``enterSelection``;
/// V3-3 T17 起快捷行区段挂载;V3-3 返工二 §14.1 起快捷行树形=成员内容
/// 合并,见类型头部「快捷行树形=成员内容合并」注)。
///
/// **复用裁定(记录)**:不另起 FavoritesViewModel——首页 VM 就是
/// ``BrowseViewModel``(浏览根 = 收件箱路径,根列表 = ``entries``,展开
/// 机制全量单源:懒加载缓存/世代守卫/缩进/排序管线与浏览页同一实现);
/// 根行本体不是 entry,其展开态 = 页面侧一个 bool——数据早已随根拉取
/// 就位(进页 ``loadRoot()`` 一次),展开/收起 = 状态翻转零拉取;收起走
/// ``collapseAll()``(世代 +1 作废在飞子项拉取、子树缓存释放——R3 评审
/// I-1 同款收口,收起不得留迟到应答复活面)。**根行不可选**(R5b #1):
/// 收件箱根行不在 ``visibleRows`` 内(非 entry),选择圈/全选/收集天然
/// 不及根——选择面只覆盖子行。
///
/// **快捷树展开态**(§14.1,视图侧薄状态——扇出纪律与混合视图
/// ``MixedViewModel/reload`` 同源,无独立 VM 面无持久状态,全现场派生):
/// fav 级箭头翻转 = ``expandedFavIds``;展开即种子扇出——设备快照判离线
/// 成员零请求(条目缺席),在线成员未缓存即拉(在飞先入 ``loadingKeys``,
/// 渐进到达;``attemptedRoots`` 记本世代已尝试,.failed 派生口径 = 在线 +
/// 无缓存 + 不在飞 + 已尝试)、已缓存零拉取即时并入(重展开 = 新世代:
/// 收起释放缓存,失败成员重试归重展);目录行再展 = 懒加载该目录在其设备
/// 上的子级(``expandedNodes`` 行链身份、缓存键 device\0path 与根档同空间,
/// 收起按引用释放——同收件箱 collapseAll 语义,收起不留复活面)。首层合并
/// 排序 = Kit 纯投影 ``FavoritesTree`` 单源(目录恒前+名称升序,比较器 =
/// ``BrowseViewModel/less``);下拉刷新逐键现拉缓存(收件箱
/// ``refreshExpandedChildren`` 同款收口);快捷整单变更(管理屏编辑)时
/// 已展开条目增量重种子。树行不进收件箱选择集(非 entry),选择态点按
/// 全部无操作;文件行无点按无滑面。
private struct FavoritesRootList: View {
    let repo: MtdssRepository
    let selfDeviceId: String?
    let inbox: AuthorizedRoot
    /// 快捷 store(V3-3 T14;@ObservedObject 接 @Published 失效面——清单/
    /// readOnly/busy 行面直读)。
    @ObservedObject private var favorites: FavoritesStore
    var showHidden: Bool
    let onOpenRoot: (AuthorizedRoot) -> Void
    /// 子目录行点按 = 直入该子目录整页(R4 #1 用户裁定回炉:单目的,
    /// 子页 VM 以子目录为浏览根 → 顶栏返回/边缘右滑弹回首页,无中间
    /// 收件箱页;文件行无点按面,同浏览页)。
    let onOpenPath: (String) -> Void
    /// 快捷行点按 = 推 .fav 混合页(T16;FavoritesView navPath 收口)。
    let onOpenFavorite: (Favorite) -> Void
    /// 成员行/其行内子目录行点按 = 推成员目录整页浏览(§13.8;navPath
    /// 收口:fav = 路由上下文,member.device = 浏览目标,path = 目标路径)。
    let onOpenMember: (Favorite, FavMember, String) -> Void

    @StateObject private var vm: BrowseViewModel
    @State private var inboxExpanded = false
    /// 快捷树展开态(§14.1,语义见类型头部「快捷树展开态」注):
    /// 已展开的快捷条目(fav.id;fav 箭头翻转态,收起 = 合并子树整片收拢)。
    @State private var expandedFavIds: Set<String> = []
    /// 已展开的目录节点(键 = 行链身份 favId\0成员位次\0device\0path 起,
    /// 下级续接 device\0path;跨快捷同目录各自独立展开态)。仅拉取成功后
    /// 入集(toggleExpand 语义同源)。
    @State private var expandedNodes: Set<String> = []
    /// 该设备某路径子级全量缓存(键 = device\0path;快捷根档与树内目录
    /// 子级同一缓存空间,跨快捷/跨节点共享一份,一次展开一次拉取,收起
    /// 按引用释放;缓存存全量条目,渲染层滤隐藏——Windows 同裁定)。
    @State private var memberChildren: [String: [DirEntry]] = [:]
    /// 路径拉取在飞(键 = device\0path;防重复,在飞点按吞掉,toggleExpand
    /// 同款;.pending 派生源之一)。
    @State private var loadingKeys: Set<String> = []
    /// 本世代已尝试的根档键(键 = fav.id → 成员键集;.failed 派生口径参与
    /// ——在线+无缓存+不在飞+已尝试;收起清空,重展开 = 新世代即重试)。
    @State private var attemptedRoots: [String: Set<String>] = [:]
    /// DEBUG 钩子恰一次闸(.task 在本页可因 SwiftUI 生命周期更新重入——
    /// 二次执行 toggleInboxExpand 会把已展开树收拢,R6 截图轮实证)
    @State private var debugHookFired = false
    @Environment(AppModel.self) private var model
    /// 确认后待发送的选择集(non-nil → 选设备弹层)。
    @State private var pickedForSend: PickedSelection?
    /// 待确认删除请求(选择删除;确认居中弹窗 R5b #9 同款)。
    @State private var deleteRequest: DeleteRequest?
    /// 删除结果提示(部分失败明细;纯成功静默)。
    @State private var deleteNotice: String?
    @State private var sending = false
    /// 动作发起反馈胶囊(R3 #3 同语言)。
    @State private var toast: String?
    /// 待开管理屏的快捷条目(non-nil → sheet;屏内持 id 查最新,T17)。
    @State private var manageTarget: Favorite?
    /// 待确认删除的快捷条目(左滑删除快捷;确认居中 alert,解除链接文案)。
    @State private var favDeleteRequest: Favorite?
    /// 新建快捷目录对话框(「···」菜单入口;readOnly 隐藏入口)。
    @State private var showCreate = false
    @State private var newName = ""
    /// 快捷操作失败文案(新建/删除;store op 返 Bool 无明细面,plain)。
    @State private var favOpError: String?
    /// 设备快照(快捷树行离线判定 join 数据源,§13.8/§14.1;进页/下拉刷新
    /// 现拉,失败保旧值——快照未及窗/失败窗恒按离线保守呈现,刷新归位)。
    @State private var devices: [DirectoryDevice] = []

    init(
        repo: MtdssRepository,
        selfDeviceId: String?,
        inbox: AuthorizedRoot,
        favorites: FavoritesStore,
        showHidden: Bool,
        onOpenRoot: @escaping (AuthorizedRoot) -> Void,
        onOpenPath: @escaping (String) -> Void,
        onOpenFavorite: @escaping (Favorite) -> Void,
        onOpenMember: @escaping (Favorite, FavMember, String) -> Void,
    ) {
        self.repo = repo
        self.selfDeviceId = selfDeviceId
        self.inbox = inbox
        self.favorites = favorites
        self.showHidden = showHidden
        self.onOpenRoot = onOpenRoot
        self.onOpenPath = onOpenPath
        self.onOpenFavorite = onOpenFavorite
        self.onOpenMember = onOpenMember
        _vm = StateObject(wrappedValue: BrowseViewModel(
            repo: repo,
            deviceId: BrowseViewModel.localBrowseDeviceId(selfDeviceId: selfDeviceId),
            rootPath: inbox.path,
            rootTitle: inbox.name,
        ))
    }

    var body: some View {
        VStack(spacing: 0) {
            if vm.isSelecting {
                SelectionNavBar(
                    count: vm.selection.count,
                    onSelectAll: { vm.selectAll() },
                    onConfirm: { vm.confirmSelection() },
                )
            } else {
                BrowseNavBar(title: "快捷目录", showsArrows: false) {
                    overflowMenu
                }
            }
            rootList
        }
        .background(Color(uiColor: .systemBackground))
        .toolbar(.hidden, for: .navigationBar)
        // 选择模式隐藏标签栏(spec §4.2;R6 #2 根因页:本根页恒挂
        // `.toolbar(isSelecting ? .hidden : .visible)` 的 **.visible 半边**
        // 压过推入页选择的 .hidden(iOS 26 根页声明优先)——推入页进选择
        // 标签栏不藏,20261102.4 真机 + sim 双钉。条件挂载:仅本页自己在
        // 选择时声明 hidden,推入页在顶时本页零声明,单声明者胜出)
        .tabBarHiddenWhenSelecting(vm.isSelecting)
        // 动作发起反馈(首页分享发起;浏览页同语言)
        .toast($toast)
        .safeAreaInset(edge: .bottom) {
            if vm.isSelecting {
                // 本地档动作集 Kit 单源(R9 #3;isRemote:false,本地删除不设门)
                SelectionActionBar(
                    actions: BrowseSelectionAction.actions(
                        isRemote: false, allowRemoteDelete: false,
                    ).map(SelectionActionBar.ActionKind.init),
                ) { action in
                    switch action {
                    case .share: shareSelection()
                    case .delete: confirmDeleteSelection()
                    default: break
                    }
                }
            }
        }
        .sheet(item: $pickedForSend) { picked in
            DevicePickerSheet(
                title: "发送 \(picked.items.count) 项到…",
                repo: repo,
                selfDeviceId: model.selfDeviceId,
            ) { device in
                startSend(picked.items, to: device)
            }
        }
        // 删除确认(居中 alert,spec §4.2/R5b #9 同款文案;R11 ③:文件夹
        // 形文案点明整树语义,纯文件形逐字不变)
        .alert(
            deleteRequest?.alertTitle ?? "删除 0 项?",
            isPresented: Binding(
                get: { deleteRequest != nil },
                set: { if !$0 { deleteRequest = nil } },
            ),
        ) {
            Button("删除", role: .destructive) {
                if let request = deleteRequest {
                    Task { await performDelete(request.paths) }
                }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text(deleteRequest?.alertMessage ?? "")
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
        .alert(
            "删除完成",
            isPresented: Binding(
                get: { deleteNotice != nil },
                set: { if !$0 { deleteNotice = nil } },
            ),
        ) {
            Button("好", role: .cancel) {}
        } message: {
            Text(deleteNotice ?? "")
        }
        // 快捷管理屏(T17;sheet 持 id 查最新,他端删除随整单跟随自关)
        .sheet(item: $manageTarget) { fav in
            FavoriteManageSheet(favId: fav.id, repo: repo, store: favorites)
        }
        // 快捷删除确认(左滑「删除快捷」;居中 alert,解除链接文案 = 管理
        // 屏同款逐字——spec §3.1 点明绝不触碰远端文件)
        .alert(
            "删除快捷目录",
            isPresented: Binding(
                get: { favDeleteRequest != nil },
                set: { if !$0 { favDeleteRequest = nil } },
            ),
        ) {
            Button("删除", role: .destructive) {
                if let fav = favDeleteRequest {
                    Task { await deleteFavorite(fav) }
                }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text(favDeleteRequest.map { "只解除链接,不删除任何文件。删除 \($0.name)?" } ?? "")
        }
        // 新建快捷目录(命名对话框;空名禁提交——钮 disabled,WWDC22
        // alert+TextField 官方形态,SettingsView 设备名同款)
        .alert("新建快捷目录", isPresented: $showCreate) {
            TextField("名称", text: $newName)
            Button("创建") {
                let name = newName.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !name.isEmpty else { return }
                Task { await createFavorite(name) }
            }
            .disabled(newName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            Button("取消", role: .cancel) {}
        } message: {
            Text("创建后可把设备里的目录添加进来")
        }
        // 快捷操作失败(新建/删除;store op 返 Bool 无明细面,plain 文案)
        .alert(
            "操作失败",
            isPresented: Binding(
                get: { favOpError != nil },
                set: { if !$0 { favOpError = nil } },
            ),
        ) {
            Button("好", role: .cancel) {}
        } message: {
            Text(favOpError ?? "")
        }
    }

    // MARK: 列表(根行 + 展开子级;沉底尾注)

    private var rootList: some View {
        // R6 #3 容器迁 List(与浏览页同 ``BrowseList``;子行原生左滑与浏览
        // 行同源——自绘拖拽与滚动争用真机恒败)。尾注 = 普通末行(沉底
        // 语义取舍记录在 ``BrowseList`` 头)
        BrowseList {
            inboxRow
            if inboxExpanded {
                childrenArea
            }
            favoritesSection
            ItemCountFooter(count: 1 + favorites.favorites.count)
        }
        // 根行展开/收起与子级行内展开各驱动各的插拔/旋转动画(值 scoped,
        // 与 BrowseView 同拍 0.2s);快捷树(fav 级/节点级/缓存到达)同拍;
        // 选择态进/出驱动选圈列宽动画(同 R4 #3)
        .animation(.easeInOut(duration: 0.2), value: inboxExpanded)
        .animation(.easeInOut(duration: 0.2), value: vm.expandedPaths)
        .animation(.easeInOut(duration: 0.2), value: vm.isSelecting)
        .animation(.easeInOut(duration: 0.2), value: expandedFavIds)
        .animation(.easeInOut(duration: 0.2), value: expandedNodes)
        .animation(.easeInOut(duration: 0.2), value: memberChildren)
        // 下拉刷新(R4 真机轮 #4;R5a 修#4):status 重投影(根数据源)+
        // 收件箱原地重载(不动历史/展开态保持,子缓存逐个现拉刷新)+
        // 设备快照/快捷整单重拉(T17;refreshStatus 登录态已带 favorites
        // refresh,显式再拉 = 未登录窗也尝试,store 失败保旧值)+ 快捷树
        // 缓存逐键现拉(§14.1;refreshExpandedChildren 同款收口);
        // 非结构化 Task 承载解绑 SwiftUI 取消(R5a #4,三面同款)
        .refreshable {
            await Task {
                await model.refreshStatus()
                await vm.reload()
                await loadDevices()
                await favorites.refresh(repo: repo)
                await refreshFavTree()
            }.value
        }
        .task {
            // 设备快照(成员行离线判定 join;进页一次,失败保旧值)
            await loadDevices()
            // 根列表(收件箱直接子项)进页即拉一次:根行展开零等待(子级的
            // 子级仍走 toggleExpand 懒加载);失败不阻塞首页(展开后行内重试)
            if vm.entries.isEmpty && vm.errorText == nil && !vm.loading {
                await vm.loadRoot()
            }
        }
        .task { vm.showHidden = showHidden }
        .onChange(of: showHidden) { _, hidden in
            vm.showHidden = hidden
        }
        // 快捷整单变更(管理屏编辑/他端写回,T17 面单源):已展开条目增量
        // 重种子(新成员即拉,移除成员的行随派生消失);已删除条目的展开态
        // 与缓存引用一并清理(§14.1 fav 变更世代纪律)。
        .onChange(of: favorites.favorites) { _, newValue in
            let liveIds = Set(newValue.map(\.id))
            for fav in newValue where expandedFavIds.contains(fav.id) {
                seedFavoriteRoots(fav)
            }
            pruneStaleFavorites(liveIds)
        }
        #if DEBUG
        .task {
            guard !debugHookFired else { return }
            debugHookFired = true
            await debugExpandIfHooked()
            await debugFavlistIfHooked()
        }
        #endif
    }

    // MARK: 「···」菜单(选择 + 排序单选勾;选择入口其一)

    @ViewBuilder private var overflowMenu: some View {
        Button("选择") {
            vm.enterSelection()
        }
        if !favorites.readOnly {
            // 新建快捷目录(T17;readOnly 隐藏——v>1 前向守卫,store 拒写,
            // 编辑入口不示能)
            Button("新建快捷目录") {
                newName = ""
                showCreate = true
            }
        }
        // 根页 = 收件箱单枚(非文件浏览),排序无意义但保留形(T5)
        Picker("排序", selection: .constant(BrowseSort.name)) {
            ForEach(BrowseSort.allCases) { field in
                Text(field.label).tag(field)
            }
        }
        .disabled(true)
    }

    // MARK: 选择流(spec §4.2 + Ruling O:先捕获后退出;动作仅自底栏)

    /// 底栏分享 = 捕获选择集 → 退出选择模式 → 选设备弹层(浏览页同款次序)。
    private func shareSelection() {
        let items = vm.selectedEntries
        guard !items.isEmpty else { return }
        vm.exitSelection()
        pickedForSend = PickedSelection(items: items)
    }

    /// 底栏删除 = 捕获路径入确认弹窗(确认后才执行;R5b #9。R11 ③:目录
    /// 计数随行——文件夹形文案点明整树语义)。
    private func confirmDeleteSelection() {
        let entries = vm.selectedEntries
        guard !entries.isEmpty else { return }
        deleteRequest = DeleteRequest(
            paths: entries.map(\.path),
            folders: entries.filter(\.isDir).count)
    }

    /// 删除执行(确认后;R5b #9):选择档走 ``deleteSelected``(退选+原地
    /// 重载收口在 VM),滑删档走 ``deletePaths``+原地重载;部分失败给明细,
    /// 全成静默。
    private func performDelete(_ paths: [String]) async {
        if vm.isSelecting {
            guard let result = await vm.deleteSelected() else { return } // 失败走 lastActionError
            guard !result.failed.isEmpty else { return } // 全成静默
            deleteNotice = "已删除 \(result.deleted) 项,\(result.failed.count) 项失败:\(result.failed[0].message)"
        } else {
            // 行滑删(R5b #7):单条目,device = 本机(home VM 浏览目标)
            guard let result = await vm.deletePaths(paths) else { return } // 失败走 lastActionError
            await vm.reload()
            guard !result.failed.isEmpty else { return } // 全成静默
            deleteNotice = "已删除 \(result.deleted) 项,\(result.failed.count) 项失败:\(result.failed[0].message)"
        }
    }

    /// 发送到设备(选设备弹层点选后;选择态已在动作面收)。发起成功
    /// toast(R3 #3);失败提示,dir = 收件箱哨兵(R12 ① 全平台口径)。
    /// files = 收件箱内绝对路径——引擎上传源守卫按授权根收口,收件箱恒在
    /// 授权集(spec §5.2),与本地浏览页发送同形。
    private func startSend(_ items: [DirEntry], to device: DirectoryDevice) {
        let send = SendService(repo: repo)
        guard !sending else { return }
        sending = true
        let paths = items.map(\.path)
        Task {
            defer { sending = false }
            do {
                _ = try await send.send(paths: paths, to: device)
                toast = "已开始 \(paths.count) 项 · 进度见传输列表"
            } catch {
                vm.lastActionError = BrowseViewModel.userErrorText(for: error)
            }
        }
    }

    // MARK: 根行(收件箱:箭头 = 展开,行点按 = 进整页;选择态不可选)

    private var inboxRow: some View {
        Button {
            // 选择模式根行不可选也不导航(R5b #1:点按 = 无操作,不误出页)
            guard !vm.isSelecting else { return }
            onOpenRoot(inbox)
        } label: {
            FullBleedRow {
                HStack(spacing: 12) {
                    // 箭头 = 行内展开钮(R4 #1):› 展开态旋转 90° 成 ∨;
                    // 触区外扩 28pt 方 + 负 padding 还原占位(BrowseEntryRow
                    // 同手法,视觉像素不变);选择态收起钮同禁(展开面照旧
                    // 可用与否无碍选择——保留展开便于子行勾选,R5b 语义)
                    Button {
                        toggleInboxExpand()
                    } label: {
                        Image(systemName: "chevron.right")
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundStyle(Color.accentColor)
                            .frame(width: 12)
                            .rotationEffect(.degrees(inboxExpanded ? 90 : 0))
                            .padding(8)
                            .contentShape(Rectangle())
                            .padding(.horizontal, -8)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(Text(inboxExpanded ? "收起" : "展开"))
                    RootFolderIcon()
                    Text(inbox.name)
                        .font(.system(size: 17))
                        .foregroundStyle(Color.primary)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 20)
            }
            // 整行触区(R3 真机轮 #1/#5:根行同浏览行,透明区可点)
            .contentShape(Rectangle())
        }
        .buttonStyle(PressableRowStyle())
        // 根行无选圈(R5b #1:收件箱根 = 容器行,非文件条目;不进选择集,
        // 不给圈——选择面只覆盖子行)
        .accessibilityLabel(Text(inbox.name))
    }

    /// 根行箭头点按(R4 #1):数据早已随根拉取就位,展开/收起 = 状态翻转
    /// 零拉取;收起走 ``collapseAll()``(见类型注释)。
    private func toggleInboxExpand() {
        if inboxExpanded {
            vm.collapseAll()
            inboxExpanded = false
        } else {
            inboxExpanded = true
        }
    }

    // MARK: 快捷行树(V3-3 返工二 §14.1:收件箱根行下方 = 快捷条目行;
    //        箭头 = 行内展开成员内容并集(在线成员列表并集,文件+目录混排、
    //        行带来源色点),行点按 = 推 .fav 混合页;左滑 = [管理, 删除
    //        快捷];选择态无点按无滑面——树行均非收件箱 entry,不进选择集)

    @ViewBuilder private var favoritesSection: some View {
        if favorites.favorites.isEmpty {
            // 空清单 hint(非 readOnly 才出——前向守卫下无新建面,不诱编辑)
            if !favorites.readOnly {
                emptyHintRow
            }
        } else {
            ForEach(favorites.favorites) { fav in
                favoriteRow(fav)
                if expandedFavIds.contains(fav.id) {
                    favTreeArea(fav)
                }
            }
        }
    }

    /// 空清单引导行(muted 不可点,非 Button 无按压反馈)。
    private var emptyHintRow: some View {
        FullBleedRow {
            Text("新建快捷目录,再把设备里的目录添加进来")
                .font(.system(size: 14))
                .foregroundStyle(Color(uiColor: .systemGray))
                .padding(.horizontal, 20)
        }
        .accessibilityElement(children: .combine)
    }

    /// 快捷条目行:箭头 = 行内展开成员内容并集区(§14.1;有成员才可展开,
    /// 空成员 = 隐形占位对齐行名——BrowseEntryRow 文件行手法,不示死钮);
    /// 点行 = 推 .fav 混合页(选择态无点按);左滑 = [管理, 删除快捷]
    /// (readOnly 翻空集——前向守卫禁编辑,管理/删除入口一并收)。
    private func favoriteRow(_ fav: Favorite) -> some View {
        let swipes = (vm.isSelecting || favorites.readOnly) ? [] : FavoriteSwipeAction.all
        return Button {
            // 选择态点按 = 无操作(不导航不勾选,根行同款 R5b #1 语义)
            guard !vm.isSelecting else { return }
            onOpenFavorite(fav)
        } label: {
            FullBleedRow {
                HStack(spacing: 12) {
                    favoriteCaret(fav)
                    RootFolderIcon()
                    Text(fav.name)
                        .font(.system(size: 17))
                        .foregroundStyle(Color.primary)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 20)
            }
            // 整行触区(R3 真机轮 #1/#5 同款:透明区可点)
            .contentShape(Rectangle())
        }
        .buttonStyle(PressableRowStyle())
        .modifier(NativeSwipeActions(
            actions: swipes,
            spec: { $0.swipeSpec },
            onAction: { action in
                switch action {
                case .manage:
                    manageTarget = fav
                case .delete:
                    favDeleteRequest = fav
                }
            },
        ))
        .accessibilityLabel(Text(fav.name))
    }

    /// 快捷行箭头(同收件箱根行:› 展开态旋转 90° 成 ∨,触区外扩 28pt 方
    /// + 负 padding 还原布局占位;无成员 = 12pt 隐形占位,行名左缘对齐不
    /// 漂)。选择态照旧可展开/收起(展开面合并扇出惰性,同根行 R5b 语义)。
    @ViewBuilder private func favoriteCaret(_ fav: Favorite) -> some View {
        if fav.members.isEmpty {
            Image(systemName: "chevron.right")
                .font(.system(size: 14, weight: .semibold))
                .frame(width: 12)
                .opacity(0)
        } else {
            Button {
                toggleFavoriteExpand(fav)
            } label: {
                Image(systemName: "chevron.right")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(Color.accentColor)
                    .frame(width: 12)
                    .rotationEffect(.degrees(expandedFavIds.contains(fav.id) ? 90 : 0))
                    .padding(8)
                    .contentShape(Rectangle())
                    .padding(.horizontal, -8)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(Text(expandedFavIds.contains(fav.id) ? "收起" : "展开"))
        }
    }

    // MARK: 快捷树合并区(§14.1:首层 = 各在线成员列表并集,Kit 纯投影
    //        ``FavoritesTree`` 单源;目录行再展 = 该目录在其设备上的子级,
    //        递归;文件行仅展示不响应;离线/失败成员缺席,定局后尾部注记)

    /// 快捷树区(快捷行展开子级):合并扇出在飞 = 转圈行(行渐进出现不等
    /// 齐,Windows FavTreeRows 同位);注记行 = 「N 个成员离线」(定局后才
    /// 出——pending 未定局不计,条目仍可渐进到达)。
    @ViewBuilder private func favTreeArea(_ fav: Favorite) -> some View {
        let states = favMemberStates(fav)
        let loading = states.contains { $0.status == .pending }
        if loading {
            favTreeSpinnerRow(1)
        }
        ForEach(favTreeRows(fav, states: states)) { row in
            favTreeRowView(fav, row)
        }
        let absent = FavoritesTree.unavailableCount(states)
        if absent > 0 && !loading {
            favTreeFootnoteRow(absent)
        }
    }

    /// 快捷树扁平行投影:首层 = ``FavoritesTree/rootRows``(目录恒前+名称
    /// 升序、复合键、离线缺席;成员位次入链身份);目录行已展 = 该目录在其
    /// 设备上的子级行随后(递归;缓存/在飞同 memberChildren/loadingKeys
    /// 单源,键 = device\0path 与根档同空间)。行身份 = 链复合(favId\0
    /// 成员位次\0device\0path 根级,下级续接 device\0path)——同路径跨源/
    /// 跨成员/重复成员身份必不同,渐进到达序稳定。
    private func favTreeRows(_ fav: Favorite, states: [MixedViewModel.MemberState]) -> [FavTreeRow] {
        var rows: [FavTreeRow] = []
        for row in FavoritesTree.rootRows(states) {
            let id = [fav.id, String(row.memberIndex), row.device, row.entry.path]
                .joined(separator: "\u{0000}")
            rows.append(FavTreeRow(
                kind: .entry, id: id, entry: row.entry, device: row.device,
                depth: 1, parentRowId: nil))
            appendFavTreeChildren(
                device: row.device, path: row.entry.path,
                parentId: id, depth: 2, into: &rows)
        }
        return rows
    }

    /// 目录子级扁平行(递归;§14.1 子目录节点再展开 = 该子目录在其所属设备
    /// 上的子级,文件各级可见):已缓存 = 排序行随后(空目录 = 「空文件夹」
    /// 注记行,Windows emptyDir 同词);未缓存在飞 = 转圈行(收件箱 loading
    /// 行同款位);未缓存不在飞 = 不出(不应发生——展开仅在成功后入集)。
    private func appendFavTreeChildren(
        device: String, path: String,
        parentId: String, depth: Int, into rows: inout [FavTreeRow],
    ) {
        guard expandedNodes.contains(parentId) else { return }
        let cacheKey = device + "\u{0000}" + path
        guard let children = memberChildren[cacheKey] else {
            if loadingKeys.contains(cacheKey) {
                rows.append(FavTreeRow(
                    kind: .loading, id: parentId + "\u{0000}loading", entry: nil,
                    device: device, depth: depth, parentRowId: parentId))
            }
            return
        }
        let visible = children.filter { showHidden || !$0.name.hasPrefix(".") }
        for entry in BrowseViewModel.sorted(visible, by: .name) {
            let id = parentId + "\u{0000}" + device + "\u{0000}" + entry.path
            rows.append(FavTreeRow(
                kind: .entry, id: id, entry: entry, device: device,
                depth: depth, parentRowId: parentId))
            appendFavTreeChildren(
                device: device, path: entry.path,
                parentId: id, depth: depth + 1, into: &rows)
        }
        if visible.isEmpty {
            rows.append(FavTreeRow(
                kind: .emptyHint, id: parentId + "\u{0000}empty", entry: nil,
                device: device, depth: depth, parentRowId: parentId))
        }
    }

    /// 树行渲染:entry = ``BrowseEntryRow`` 复用(来源色点 §14.1,色 =
    /// ``FavoritesPalette`` 加入顺序位次;目录行箭头 = 行内展开,点按 = 推
    /// 成员目录整页浏览;文件行仅展示不响应——onOpen 对文件零动作,与浏览
    /// 页文件行同形;选择态点按 = 无操作,树行非收件箱 entry 不进选择集);
    /// loading / emptyHint = 注记行(muted,无交互)。
    @ViewBuilder private func favTreeRowView(_ fav: Favorite, _ row: FavTreeRow) -> some View {
        switch row.kind {
        case .loading:
            favTreeSpinnerRow(row.depth)
        case .emptyHint:
            Text("空文件夹")
                .font(.system(size: 13))
                .foregroundStyle(Color(uiColor: .systemGray))
                .padding(.leading, 20 + BrowseViewModel.indentPerLevel * CGFloat(row.depth))
                .padding(.trailing, 20)
                .frame(height: 44)
                .accessibilityElement(children: .combine)
        case .entry:
            if let entry = row.entry {
                BrowseEntryRow(
                    entry: entry,
                    depth: row.depth,
                    expanded: expandedNodes.contains(row.id),
                    sourceDot: sourceDot(row.device),
                    onOpen: {
                        // 文件行仅展示不响应(§14.1);目录行 = 推成员目录
                        // 整页浏览(member 路由,member = 行目标重建——页面
                        // 数据面只取 device+path)
                        guard !vm.isSelecting, entry.isDir else { return }
                        onOpenMember(fav, FavMember(device: row.device, path: entry.path), entry.path)
                    },
                    onToggleExpand: entry.isDir ? { toggleFavDirNode(fav, row) } : nil,
                )
            }
        }
    }

    /// 转圈行(44pt;收件箱展开 loading 行同款位,缩进随树级)。
    private func favTreeSpinnerRow(_ depth: Int) -> some View {
        HStack {
            ProgressView()
            Spacer(minLength: 0)
        }
        .padding(.leading, 20 + BrowseViewModel.indentPerLevel * CGFloat(depth))
        .padding(.trailing, 20)
        .frame(height: 44)
    }

    /// 离线注记行(「N 个成员离线」;定局后出,计数 = offline + failed,
    /// Kit ``FavoritesTree/unavailableCount`` 单源)。
    private func favTreeFootnoteRow(_ count: Int) -> some View {
        Text("\(count) 个成员离线")
            .font(.system(size: 13))
            .foregroundStyle(Color(uiColor: .systemGray))
            .padding(.leading, 20 + BrowseViewModel.indentPerLevel)
            .padding(.trailing, 20)
            .frame(height: 44)
            .accessibilityElement(children: .combine)
    }

    /// 行首来源色点(§14.1 合并行;色 = ``FavoritesPalette`` 加入顺序位次
    /// 映射,快照未及 = 不渲染——合并行只在在线成员有条目时出现,快照已在)。
    private func sourceDot(_ device: String) -> Color? {
        guard let hex = FavoritesPalette.colorMap(devices: devices)[device] else { return nil }
        return Color(paletteHex: hex)
    }

    // MARK: 快捷树展开态(§14.1;机制镜像收件箱根行与 ``BrowseViewModel``
    //        展开口 + 混合视图扇出纪律:懒加载缓存、收起释放、在飞吞点、
    //        失败不展开、offline 零请求、渐进并入、世代守卫)

    /// 成员键(device + NUL + path;与 store memberKey 同象)。
    private func memberKey(_ member: FavMember) -> String {
        member.device + "\u{0000}" + member.path
    }

    /// 成员在线判定(devices 快照 join;快照查无此设备 = 离线,MixedViewModel
    /// 分桶同象;快照未及窗恒保守离线,刷新/重进页归位)。
    private func isMemberOnline(_ member: FavMember) -> Bool {
        devices.contains { $0.deviceId == member.device && $0.online }
    }

    /// 成员派生状态(无持久状态面——devices 快照 + memberChildren +
    /// loadingKeys + attemptedRoots 现场推导;``MixedViewModel/MemberState``
    /// 复用为 Kit 纯投影输入面,entries 先行隐藏过滤)。.failed 口径 = 在线
    /// + 无缓存 + 不在飞 + 已尝试(新成员从未尝试 → .pending 呈现,变更
    /// 重种子即拉,零 .failed 闪面)。
    private func favMemberStates(_ fav: Favorite) -> [MixedViewModel.MemberState] {
        let attempted = attemptedRoots[fav.id] ?? []
        return fav.members.map { member in
            let key = memberKey(member)
            if !isMemberOnline(member) {
                return .init(member: member, status: .offline, entries: [])
            }
            if let entries = memberChildren[key] {
                return .init(member: member, status: .ok, entries: visibleEntries(entries))
            }
            if loadingKeys.contains(key) || !attempted.contains(key) {
                return .init(member: member, status: .pending, entries: [])
            }
            return .init(member: member, status: .failed, entries: [])
        }
    }

    /// 隐藏过滤(浏览页同口径;缓存存全量条目,渲染层滤——Windows 同裁定)。
    private func visibleEntries(_ entries: [DirEntry]) -> [DirEntry] {
        entries.filter { showHidden || !$0.name.hasPrefix(".") }
    }

    /// 快捷行箭头点按:展开 = 种子扇出(离线零请求;在线未缓存即拉、已缓存
    /// 零拉取即时并入)——重展开 = 新世代(收起已释放缓存/attemptedRoots,
    /// 失败成员即重试);收起 = 合并子树整片收拢 + 缓存按引用释放(同收件箱
    /// collapseAll 语义——收起不留复活面;跨快捷共享仍引用则留)。
    private func toggleFavoriteExpand(_ fav: Favorite) {
        if expandedFavIds.contains(fav.id) {
            collapseFavorite(fav)
        } else {
            expandedFavIds.insert(fav.id)
            seedFavoriteRoots(fav)
        }
    }

    /// 展开种子(成员内容合并扇出,``MixedViewModel/reload`` 分桶纪律同源):
    /// 设备快照判离线的成员零请求(条目缺席,注记计数);在线成员 = 已缓存
    /// 即时并入(零拉取),未缓存即拉(在飞标记先入 loadingKeys——渲染先
    /// 于 Task 排队,零 .failed 闪面)。已尝试且失败(无缓存不在飞)的成员
    /// 不随变更重种子自动重试——重试归收起重展(新世代)或下拉刷新;仅新
    /// 成员(成员集变更)即拉。
    private func seedFavoriteRoots(_ fav: Favorite) {
        var attempted = attemptedRoots[fav.id] ?? []
        for member in fav.members {
            guard isMemberOnline(member) else { continue } // 离线:零请求
            let key = memberKey(member)
            let isNew = !attempted.contains(key)
            attempted.insert(key)
            guard isNew else { continue }
            guard memberChildren[key] == nil, !loadingKeys.contains(key) else { continue }
            loadingKeys.insert(key)
            spawnListingFetch(favId: fav.id, device: member.device, path: member.path, parentRowId: nil, nodeId: nil)
        }
        attemptedRoots[fav.id] = attempted
    }

    /// 目录行箭头点按(镜像 ``BrowseViewModel/toggleExpand``):展开 = 已
    /// 缓存即时入集,否则拉取一次、成功才入集(失败不展开,行仍在可再点,
    /// 错误不打断首页主面);在飞点按吞掉;收起 = 出集 + 缓存按引用释放。
    private func toggleFavDirNode(_ fav: Favorite, _ row: FavTreeRow) {
        let key = row.cacheKey
        if expandedNodes.contains(row.id) {
            expandedNodes.remove(row.id)
            releaseCacheKeyIfUnreferenced(key)
            return
        }
        guard !loadingKeys.contains(key) else { return } // 在飞:本次点按吞掉
        if memberChildren[key] != nil {
            expandedNodes.insert(row.id) // 缓存命中即时展开,零拉取
            return
        }
        guard let entry = row.entry else { return }
        loadingKeys.insert(key)
        spawnListingFetch(favId: fav.id, device: row.device, path: entry.path, parentRowId: row.parentRowId, nodeId: row.id)
    }

    /// 单路径拉取并写缓存(根档/目录子级共用;失败 = 缓存不写——节点不
    /// 展开、行仍在可再点,根档派生 .failed 入注记;在飞标记由调用方收口)。
    /// 应答落地守卫:节点行须 fav 仍展开 + 父链仍在展开(拉取窗内断链不
    /// 复活展开面,R3 评审 I-1 同旨;缓存照写,重展开缓存命中零等待);根档
    /// 无节点身份(nodeId = nil)缓存即行。限时 30s(浏览页/混合页成员拉取
    /// 同值口径)。
    private func spawnListingFetch(favId: String, device: String, path: String, parentRowId: String?, nodeId: String?) {
        let key = device + "\u{0000}" + path
        Task {
            defer { loadingKeys.remove(key) }
            let result = try? await RpcGuard.withTimeout(seconds: 30) { [repo] in
                try await repo.list(device: device, path: path)
            }
            guard let entries = result?.entries else { return }
            memberChildren[key] = entries
            guard let nodeId, expandedFavIds.contains(favId) else { return }
            guard let parentRowId, expandedNodes.contains(parentRowId) else { return }
            expandedNodes.insert(nodeId)
        }
    }

    /// 收起快捷(合并子树整片收拢):本 fav 节点出集 + 本 fav 引用键(成员
    /// 根键 + 已展节点缓存键)按引用计数释放(其他已展开快捷仍引用则留);
    /// attemptedRoots 清空(重展开 = 新世代)。
    private func collapseFavorite(_ fav: Favorite) {
        expandedFavIds.remove(fav.id)
        let prefix = fav.id + "\u{0000}"
        let collapsed = expandedNodes.filter { $0.hasPrefix(prefix) }
        expandedNodes.subtract(collapsed)
        var candidates = Set(fav.members.map(memberKey))
        for node in collapsed {
            candidates.insert(cacheKeyOfNode(node))
        }
        let retained = referencedCacheKeys()
        for key in candidates where !retained.contains(key) {
            memberChildren[key] = nil
        }
        attemptedRoots[fav.id] = nil
    }

    /// 现存缓存键引用面(已展开快捷的成员根键 + 已展开节点缓存键;引用计数
    /// 收口单源——跨快捷共享键仍被引用则留)。
    private func referencedCacheKeys() -> Set<String> {
        var keys: Set<String> = []
        for fav in favorites.favorites where expandedFavIds.contains(fav.id) {
            for member in fav.members {
                keys.insert(memberKey(member))
            }
        }
        for node in expandedNodes {
            keys.insert(cacheKeyOfNode(node))
        }
        return keys
    }

    /// 单键引用计数释放(节点收起;无引用面才释放缓存)。
    private func releaseCacheKeyIfUnreferenced(_ key: String) {
        guard !referencedCacheKeys().contains(key) else { return }
        memberChildren[key] = nil
    }

    /// 节点链身份 → 缓存键(末两段 = device\0path;链段 NUL 分隔,favId/
    /// 成员位次前缀不影响——路径不含 NUL,与 memberKey 同象)。
    private func cacheKeyOfNode(_ nodeId: String) -> String {
        nodeId.split(separator: "\u{0000}", omittingEmptySubsequences: false)
            .suffix(2)
            .joined(separator: "\u{0000}")
    }

    /// 已删除快捷的残留展开态清理(整单变更收口):条目展开态/attempted
    /// 随 id 移除,缓存全量引用计数释放(已删除条目的成员键/节点键不再入
    /// 引用面,无引用即释放)。
    private func pruneStaleFavorites(_ liveIds: Set<String>) {
        let staleIds = expandedFavIds.subtracting(liveIds)
        expandedFavIds.subtract(staleIds)
        for stale in staleIds {
            let prefix = stale + "\u{0000}"
            expandedNodes.subtract(expandedNodes.filter { $0.hasPrefix(prefix) })
            attemptedRoots[stale] = nil
        }
        for key in Array(memberChildren.keys) where !referencedCacheKeys().contains(key) {
            memberChildren[key] = nil
        }
    }

    /// 下拉刷新跟随:缓存键逐个现拉(缓存只存被引用键——已展开快捷根档 +
    /// 已展开节点子级;收件箱 reload → ``refreshExpandedChildren`` 同款
    /// 收口)。失败 = 缓存释放 + 该键展开节点一并收起(行消失,重试归点按;
    /// 根档失败归 .failed 注记);键即缓存空间,跨快捷共享键天然只刷一次。
    private func refreshFavTree() async {
        for key in Array(memberChildren.keys) {
            let parts = key.split(separator: "\u{0000}", omittingEmptySubsequences: false).map(String.init)
            guard parts.count == 2 else { continue }
            let result = try? await RpcGuard.withTimeout(seconds: 30) { [repo] in
                try await repo.list(device: parts[0], path: parts[1])
            }
            if let entries = result?.entries {
                memberChildren[key] = entries
            } else {
                memberChildren[key] = nil
                expandedNodes.subtract(expandedNodes.filter { cacheKeyOfNode($0) == key })
            }
        }
    }

    // MARK: 快捷操作流(T17;store op 全返 Bool——put 成功才落 @Published,
    //        行面即达即反馈;失败保旧值,plain alert)

    /// 新建快捷(命名对话框提交;尾部追加,空成员合法)。
    private func createFavorite(_ name: String) async {
        guard await favorites.create(repo: repo, name: name) else {
            favOpError = "操作失败,请重试。"
            return
        }
        // 成功:行随 store 更新出现,零额外反馈(brief 只设失败 alert)
    }

    /// 删除快捷 = 解除链接(spec §3.1,绝不触碰远端文件):成功 toast,行随
    /// store 更新消失。
    private func deleteFavorite(_ fav: Favorite) async {
        guard await favorites.delete(repo: repo, id: fav.id) else {
            favOpError = "操作失败,请重试。"
            return
        }
        toast = "已删除快捷目录"
    }

    /// 设备快照现拉(DevicePickerSheet 同手法;失败保旧值——点灰,下拉
    /// 刷新/重进页收口)。
    private func loadDevices() async {
        let directory = try? await RpcGuard.withTimeout(seconds: 10) { [repo] in
            try await repo.devices()
        }
        if let directory {
            devices = directory.devices
        }
    }

    // MARK: 展开子级(depth = 行序 +1,根行占 depth 0;BrowseEntryRow 复用;
    //        R5b #1 选择模式:子行可选,长按任意子行 = 选择入口其二)

    @ViewBuilder private var childrenArea: some View {
        if let error = vm.errorText, vm.entries.isEmpty {
            // 根拉取失败:行内错误行(重试),不打断首页主面
            HStack(spacing: 8) {
                Text("无法加载目录")
                    .font(.system(size: 13))
                    .foregroundStyle(Color(uiColor: .systemGray))
                Spacer(minLength: 0)
                Button("重试") { Task { await vm.reload() } }
                    .font(.system(size: 13))
            }
            .padding(.leading, 20 + BrowseViewModel.indentPerLevel)
            .padding(.trailing, 20)
            .frame(height: 44)
        } else if vm.loading && vm.entries.isEmpty {
            HStack {
                ProgressView()
                Spacer(minLength: 0)
            }
            .padding(.leading, 20 + BrowseViewModel.indentPerLevel)
            .padding(.trailing, 20)
            .frame(height: 44)
        } else {
            ForEach(vm.visibleRows) { row in
                swipeableChildRow(row)
            }
        }
    }

    // MARK: 子行左滑(R6 #3 原生 swipeActions;R5b #7 本地行 = [发送, 删除];
    //        R11 ③ 目录子行同集[整树发送/删除];收件箱根行恒不可滑——根行
    //        非 entry 不经动作集,自绘行亦不挂滑面,结构面)

    /// 子行挂 ``NativeSwipeActions``(List 行;与浏览页同组件):动作与
    /// 选择底栏同流——发送进选设备弹层,删除入确认弹窗(目录 = 整树)。
    @ViewBuilder private func swipeableChildRow(_ row: BrowseRow) -> some View {
        // 本地集(首页仅本地档;远端删除门 R9 #3 与本地无涉,显式传 false)
        let swipes = vm.isSelecting ? [] : BrowseSwipeAction.actions(
            entry: row.entry, isRemote: false, allowRemoteDelete: false)
        BrowseEntryRow(
            entry: row.entry,
            depth: row.depth + 1,
            expanded: vm.expandedPaths.contains(row.entry.path),
            selecting: vm.isSelecting,
            selected: vm.selection.contains(row.entry.path),
            onOpen: {
                // 选择态点按 = 切换选中(不开目录,spec §4.2);
                // 常态行点按 = 进整页(目录;文件行无点按面,同浏览页)
                if vm.isSelecting {
                    vm.toggle(row.entry.path)
                } else if row.entry.isDir {
                    onOpenPath(row.entry.path)
                }
            },
            onToggleExpand: row.entry.isDir
                ? { Task { await vm.toggleExpand(row.entry.path) } }
                : nil,
        )
        .modifier(NativeSwipeActions(
            actions: swipes,
            spec: { $0.swipeSpec },
            onAction: { action in
                switch action {
                case .send:
                    pickedForSend = PickedSelection(items: [row.entry])
                case .delete:
                    // R11 ③:目录行同集(整树删除;文件夹形确认文案)
                    deleteRequest = DeleteRequest(
                        paths: [row.entry.path], folders: row.entry.isDir ? 1 : 0)
                default: break // 首页仅本地集([发送,删除])
                }
            },
        ))
        // 长按任意子行进选择(R5b #1 入口其二;BrowseView 同款 0.4s)
        .simultaneousGesture(
            LongPressGesture(minimumDuration: 0.4).onEnded { _ in
                if !vm.isSelecting { vm.enterSelection() }
            },
        )
    }

    // MARK: DEBUG 截图钩子(模拟器无触点驱动;真机/产线不编译)

    #if DEBUG
    /// MTDSS_DEBUG_FAVTREE=1:展开收件箱根行 + 其首个子目录(两级行内
    /// 展开截图面)。MTDSS_DEBUG_FAVSELECT=1:同上再进选择态并勾 3 个
    /// 子行(R5b #1 首页选择截图面:选圈列/选择顶栏/底部动作条/标签栏
    /// 隐藏)。等待口径 = BrowseView ``waitForEntries`` 同款(只看
    /// entries/error,不看 loading——loading 翻转先于应答落地,看 loading
    /// 会提前出循环)。
    private func debugExpandIfHooked() async {
        let env = ProcessInfo.processInfo.environment
        let wantSelect = env["MTDSS_DEBUG_FAVSELECT"] == "1"
        guard env["MTDSS_DEBUG_FAVTREE"] == "1" || wantSelect else { return }
        for _ in 0..<100 where vm.entries.isEmpty && vm.errorText == nil {
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        guard !vm.entries.isEmpty else {
            print("[R4-E2E] FAVTREE FAIL: 收件箱列表空(=\(vm.errorText ?? "nil"))")
            return
        }
        toggleInboxExpand()
        if let child = vm.visibleRows.first(where: { $0.entry.isDir }) {
            await vm.toggleExpand(child.entry.path)
        }
        if wantSelect {
            vm.enterSelection()
            vm.visibleRows.prefix(3).forEach { vm.toggle($0.entry.path) }
        }
    }

    /// MTDSS_DEBUG_FAVLIST=1:等快捷清单落地后停留(T17 根列表快捷行截图
    /// 面;T16 debugWaitIfHooked 同手法——只等清单投影)。清单未建(空)
    /// 也照常收:空态 hint 行即该轮截图面。
    private func debugFavlistIfHooked() async {
        guard ProcessInfo.processInfo.environment["MTDSS_DEBUG_FAVLIST"] == "1" else { return }
        for _ in 0..<100 where favorites.favorites.isEmpty && !favorites.readOnly {
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
    }
    #endif
}

/// 快捷树扁平行(视图内投影载体;§14.1 内容树):kind = 条目 / 在飞转圈 /
/// 空目录注记;entry 仅条目行承载;device = 条目行来源设备(色点/导航);
/// depth = 缩进层级(fav 子级首层 1);parentRowId = 父行链身份(根级 nil
/// ——在飞应答落地守卫:父链仍在展开才入集,断链不复活展开面)。
private struct FavTreeRow: Identifiable {
    enum Kind { case entry, loading, emptyHint }
    let kind: Kind
    let id: String
    let entry: DirEntry?
    let device: String
    let depth: Int
    let parentRowId: String?

    /// 缓存键 = device\0path(条目行;memberChildren/loadingKeys 同空间)。
    var cacheKey: String { device + "\u{0000}" + (entry?.path ?? "") }
}

/// 快捷行左滑动作(T17;``NativeSwipeActions`` 泛型面,语义同
/// ``BrowseSwipeAction`` 但快捷行集独立 = [管理, 删除快捷],声明序末位
/// 贴缘 = 删除快捷,破坏面最右与浏览集同形)。
private enum FavoriteSwipeAction: Equatable {
    case manage
    case delete

    static let all: [FavoriteSwipeAction] = [.manage, .delete]

    var swipeSpec: SwipeActionSpec {
        switch self {
        case .manage: SwipeActionSpec(symbol: "folder.badge.gearshape", tint: .accentColor, label: "管理")
        case .delete: SwipeActionSpec(symbol: "trash", tint: Color(uiColor: .systemRed), label: "删除快捷")
        }
    }
}
