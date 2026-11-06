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
/// **快捷行树=成员行**(V3-3 返工波 4 §16.3,§13.8 成员目录节点形态回归,
/// 推翻返工二 §14.1 成员内容合并):快捷行箭头 = 行内展开**成员行**——每
/// 在线成员一行,标签 = 「设备名 - 目录名」(§16.3 词面;设备名 = devices
/// 快照 join,未及退化 id;目录名 = 路径末段显示名);行箭头 = 懒加载该
/// 目录在其所属设备上的**子目录**(仅目录——§16.3 树=成员节点后 fav 位
/// 再无文件行消费,Windows X3 同裁定,递归逐级);点成员行/子目录行 = 推
/// 成员目录整页浏览(.member 路由);**离线成员行灰显禁点禁展开**(快照
/// 缺该设备按离线;离线成员不再注记行——成员行自身灰显即披露,§14.1 尾
/// 注记「N 个成员离线」随内容并集一并废止);零成员快捷展开 = 引导占位行。
/// 点按/导航全部经 ``navigate``(NavPathGate §13.7 闸)。
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
            await favNewReproIfHooked()
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
            // V3-3 快捷混合页(T16;T17 根列表行触发推入)。onDrill = 钻取
            // 并入**外层栈**同型值路由(§14.6 二轮根修:嵌套子栈撤销,异型
            // 路径元素 comparisonTypeMismatch 断言崩面结构性消除;见
            // FavoritesRoute/.drill 注)。
            if let repo = model.repository {
                FavoritesMixedView(
                    fav: fav,
                    repo: repo,
                    selfDeviceId: model.selfDeviceId,
                    onPopToRoot: { navigate([]) },
                    onDrill: { device, deviceName, path in
                        navigate([.fav(fav), .drill(
                            fav, deviceId: device, deviceName: deviceName, path: path)])
                    },
                )
            } else {
                bootPlaceholder(title: fav.name, hint: "引擎启动完成后即可查看「\(fav.name)」")
            }
        case .drill(let fav, let deviceId, let deviceName, let path):
            // §14.6 二轮根修:混合页钻取目的地(原 FavoritesMixedView 页内
            // 子栈目的地随迁;数据面 = device+path,fav 仅路由上下文供弹回
            // 本页)。钻取 = VM 以子目录为浏览根(进页 canGoBack=false →
            // 顶栏返回/边缘右滑 = 弹回混合页);远端档动作集(下载/转发+
            // 删除门)+「设备名 | 目录名」标题(设备名 = 点按时点定格),
            // 本机档 = 本地动作集单段标题。
            if let repo = model.repository {
                let tail = BrowseFormat.displayName(ofPath: path)
                BrowseDestination(
                    root: AuthorizedRoot(name: tail, path: path, isInbox: false),
                    repo: repo,
                    deviceId: deviceId,
                    isRemote: deviceId != model.selfDeviceId,
                    deviceName: deviceId != model.selfDeviceId ? deviceName : nil,
                    onPopToRoot: { navigate([.fav(fav)]) },
                )
            } else {
                // repo-nil 占位返回 = 弹回根列表(bootPlaceholder 共享默认
                // navigate([])),与真目的地「弹回混合页」差一层——差异钉:
                // 占位仅引擎重启窗口期现(repository 一般随页存活),罕见态
                // 不为它加参分化,刷新归位即走真目的地语义。
                let tail = BrowseFormat.displayName(ofPath: path)
                bootPlaceholder(title: tail, hint: "引擎启动完成后即可浏览\(tail)")
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
    // MARK: S7 崩溃二轮复现驱动(§14.6;模拟器无触点驱动,真机/产线不编译)

    /// MTDSS_DEBUG_FAVNEW=rapid|old0|drill|member|vanish:S7「新建快捷目录
    /// 点入崩溃」复现驱动(§14.6 纪律 = 复现取证后根修;真机/产线不编译,
    /// 特此报备)。全程走产线同一代码面:store ``FavoritesStore/create``
    /// (kvGet→kvPut 真 RPC)+ ``navigate``(NavPathGate 闸)+ 混合页整套
    /// 生命周期。
    /// - **rapid**:「新建→点入」×N 轮,轮内点入延迟 0/100/300/600ms 循环
    ///   (新建后行即现即点的用户形态;MTDSS_DEBUG_FAVNEW_DELAY_MS=<n> 全轮
    ///   固定延迟),且含秒退再点(C 形态:tap→300ms→pop→150ms→再 tap);
    ///   轮间弹回。默认 8 轮(MTDSS_DEBUG_FAVNEW_ROUNDS 覆写)。
    ///   **修复前此形态任意延迟(0/150/600ms)首推必崩**(§14.6 根因,复现
    ///   矩阵钉),修复后全程存活——FavNewCrashUITests 回归锚同拍。
    /// - **old0**:边界钉——同周期 store 变更 + 点入,但点的是**旧**快捷
    ///   (隔离「新建条目」与「同周期点入」两因子;修复前与 rapid 同崩,
    ///   钉死崩面与新旧无关)。
    /// - **drill**:`.drill` 路由回归驱动(建本机成员快捷 → 推 [.fav] →
    ///   推 [.fav, .drill] 双元素路径 → 逐级弹回)——§14.6 根修改道后外层
    ///   同型路由的全形态锚(FavNewCrashUITests 不覆盖此面,harness 跑)。
    /// - **member**:建一枚带**离线成员**(假 device id)的快捷再点入——
    ///   混合页离线桶形态(0 请求,零 RPC 噪声)。
    /// - **vanish**:建→点入→转场窗内 kvPut **去掉本快捷的整单**+refresh
    ///   ——复刻「favorite 跟随弹回(onPopToRoot)撞转场在飞」的产线候选
    ///   根因面(取证后排除:服务端 kv 写者不收回声,data_put 注释钉)。
    /// 可观测面(R9 marker 同款):收尾写 `{MTDSS_HOME}/favnew-marker`,
    /// 内容 = `<mode>:<完成轮数|error>`;提前退出 = `<mode>:0:<原因>`。
    /// 登录:未登录即自登录(dev 回显码);server/email 经
    /// MTDSS_DEBUG_FAVNEW_SERVER / MTDSS_DEBUG_FAVNEW_EMAIL 注入(缺省产线
    /// +kk@test.com;本地 rig = Mac 上 --dev-echo-code 的 signaling-server),
    /// 失败写标记退出——前置登录由 harness 保证亦可。
    private func favNewReproIfHooked() async {
        let env = ProcessInfo.processInfo.environment
        guard let mode = env["MTDSS_DEBUG_FAVNEW"], !mode.isEmpty else { return }
        let home = env["MTDSS_HOME"] ?? ""
        func mark(_ text: String) {
            guard !home.isEmpty else { return }
            try? Data(text.appending("\n").utf8)
                .write(to: URL(fileURLWithPath: home).appendingPathComponent("favnew-marker"))
        }
        // 等引擎投影(15s 上界;.task 先于引擎 boot 完成是常态)
        for _ in 0..<150 where model.repository == nil {
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        guard let repo = model.repository else { mark("\(mode):0:no-repo"); return }
        // 登录(未登录自登;已登录零动作)
        if !(model.session.loggedIn ?? false) {
            let server = env["MTDSS_DEBUG_FAVNEW_SERVER"] ?? "https://www.mtdss.top"
            let email = env["MTDSS_DEBUG_FAVNEW_EMAIL"] ?? "kk@test.com"
            do {
                let rc = try await RpcGuard.withTimeout(seconds: 20) { [repo] in
                    try await repo.loginRequestCode(server: server, email: email)
                }
                guard !rc.code.isEmpty else { mark("\(mode):0:no-echo-code"); return }
                _ = try await RpcGuard.withTimeout(seconds: 20) { [repo] in
                    try await repo.loginVerify(code: rc.code, email: email, name: "iOS sim")
                }
            } catch {
                mark("\(mode):0:login-fail"); return
            }
            var logged = false
            for _ in 0..<100 {
                if model.session.loggedIn ?? false { logged = true; break }
                // 事件窗竞态兜底:status 权威态直查翻转(SecurityModel refresh 同口径)
                await model.session.refresh(repo: repo)
                if model.session.loggedIn ?? false { logged = true; break }
                try? await Task.sleep(nanoseconds: 200_000_000)
            }
            guard logged else { mark("\(mode):0:login-timeout"); return }
        }
        // 清单投影对齐(登录后显式重拉;store 保旧值口径)
        await model.favorites.refresh(repo: repo)
        switch mode {
        case "rapid":
            let rounds = Int(env["MTDSS_DEBUG_FAVNEW_ROUNDS"] ?? "") ?? 8
            let delays: [UInt64] = [0, 100_000_000, 300_000_000, 600_000_000]
            // MTDSS_DEBUG_FAVNEW_DELAY_MS=<n>:全轮固定点入延迟(ms;边界钉用
            // ——缺省 = 四档循环)
            let fixedDelayMs = Int(env["MTDSS_DEBUG_FAVNEW_DELAY_MS"] ?? "")
            var done = 0
            for i in 0..<rounds {
                let name = "压测\(i)"
                guard await model.favorites.create(repo: repo, name: name),
                      let fresh = model.favorites.favorites.last else {
                    mark("rapid:\(done):create-fail"); return
                }
                let delayNs: UInt64 = fixedDelayMs.map { UInt64($0) * 1_000_000 }
                    ?? delays[i % delays.count]
                try? await Task.sleep(nanoseconds: delayNs)
                navigate([.fav(fresh)])
                // C 形态:300ms 秒退 + 150ms 再点(混合页 .task/onReceive 在飞窗重入面)
                try? await Task.sleep(nanoseconds: 300_000_000)
                navigate([])
                try? await Task.sleep(nanoseconds: 150_000_000)
                navigate([.fav(fresh)])
                try? await Task.sleep(nanoseconds: 1_200_000_000)
                navigate([])
                try? await Task.sleep(nanoseconds: 800_000_000)
                done += 1
            }
            mark("rapid:\(done)")
        case "old0":
            // 边界钉:同周期 store 变更 + 0ms 点入,但点的是**旧**快捷(清单
            // 首枚)——隔离「新建条目」与「同周期点入」两个因子。循环用上一
            // 轮新建的条目作「旧」(轮次 ≥2 后必存在;首轮用清单既有首枚)。
            let rounds = Int(env["MTDSS_DEBUG_FAVNEW_ROUNDS"] ?? "") ?? 8
            var done = 0
            for i in 0..<rounds {
                guard await model.favorites.create(repo: repo, name: "垫片\(i)") else {
                    mark("old0:\(done):create-fail"); return
                }
                let target = i == 0
                    ? model.favorites.favorites.first
                    : model.favorites.favorites.first { $0.name == "垫片\(i - 1)" }
                guard let old = target else { mark("old0:\(done):no-target"); return }
                navigate([.fav(old)])
                try? await Task.sleep(nanoseconds: 1_200_000_000)
                navigate([])
                try? await Task.sleep(nanoseconds: 800_000_000)
                done += 1
            }
            mark("old0:\(done)")
        case "drill":
            // .drill 路由回归驱动(§14.6 二轮根修锚):建本机成员快捷(/tmp,
            // 引擎 is_self 本地列表,零网络)→ 推 [.fav] → 推 [.fav, .drill]
            // → 弹回 [.fav] → 弹回 []——外层栈同型双元素路径全形态。marker
            // 记完成轮数(FavNewCrashUITests 断言面)。
            let rounds = Int(env["MTDSS_DEBUG_FAVNEW_ROUNDS"] ?? "") ?? 4
            guard let selfId = model.session.deviceId else {
                mark("drill:0:no-self-id"); return
            }
            var done = 0
            for i in 0..<rounds {
                guard await model.favorites.create(repo: repo, name: "钻取\(i)"),
                      let fresh = model.favorites.favorites.last else {
                    mark("drill:\(done):create-fail"); return
                }
                _ = await model.favorites.addMembers(
                    repo: repo, id: fresh.id,
                    members: [FavMember(device: selfId, path: "/tmp")])
                navigate([.fav(fresh)])
                try? await Task.sleep(nanoseconds: 800_000_000)
                navigate([.fav(fresh), .drill(
                    fresh, deviceId: selfId, deviceName: nil, path: "/tmp")])
                try? await Task.sleep(nanoseconds: 1_200_000_000)
                navigate([.fav(fresh)])
                try? await Task.sleep(nanoseconds: 800_000_000)
                navigate([])
                try? await Task.sleep(nanoseconds: 800_000_000)
                done += 1
            }
            mark("drill:\(done)")
        case "member":
            guard await model.favorites.create(repo: repo, name: "离线成员压测"),
                  let fresh = model.favorites.favorites.last else {
                mark("member:0:create-fail"); return
            }
            let added = await model.favorites.addMembers(
                repo: repo, id: fresh.id,
                members: [FavMember(device: "repro-offline-zzz", path: "/tmp/repro")])
            guard added > 0 else { mark("member:0:member-fail"); return }
            try? await Task.sleep(nanoseconds: 200_000_000)
            navigate([.fav(fresh)])
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            navigate([])
            try? await Task.sleep(nanoseconds: 800_000_000)
            mark("member:1")
        case "vanish":
            guard await model.favorites.create(repo: repo, name: "弹回竞窗压测"),
                  let fresh = model.favorites.favorites.last else {
                mark("vanish:0:create-fail"); return
            }
            navigate([.fav(fresh)])
            // 转场在飞窗内(250ms)整单改写去掉本快捷 + refresh——favorite
            // 跟随的弹回路径撞转场(产线 data_changed 回声竞窗同象)
            try? await Task.sleep(nanoseconds: 250_000_000)
            let rest = model.favorites.favorites.filter { $0.id != fresh.id }
            let doc = FavoritesDoc(
                v: 1, updatedAt: Int64(Date().timeIntervalSince1970 * 1000), favorites: rest)
            if let payload = try? JSONEncoder().encode(doc) {
                _ = try? await repo.kvPut(
                    key: FavoritesStore.kvKey, value: String(decoding: payload, as: UTF8.self))
                await model.favorites.refresh(repo: repo)
            }
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            mark("vanish:1")
        default:
            mark("\(mode):0:unknown-mode")
        }
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
    /// V3-3 返工 §13.8:快捷成员目录直入浏览页(返工波 4 §16.3 起触发面 =
    /// 快捷树成员行/懒加载子目录行点按;浏览根 = 目标路径;标题 = 目录
    /// 显示名、设备隐含——与 §13.6 Windows 面包屑「不显示设备名与原始路径」
    /// 同口径;Favorite 仅路由上下文,页面数据面 = device+path,device 取
    /// 目标真实 id:引擎 is_self 认真实 id[本机成员短路同口,T13b 修#4])。
    case member(Favorite, FavMember, path: String)
    /// §14.6 二轮根修(2026-11-06):混合页钻取(原 FavoritesMixedView 页内
    /// 子 NavigationStack 值路由 FavDrillTarget 撤销——iOS 26 嵌套栈异型路径
    /// 元素 comparisonTypeMismatch 断言崩面,用户「新建快捷点入即崩」实证;
    /// 改外层栈同型值路由)。fav = 弹回上下文(onPopToRoot 归混合页);
    /// deviceName = 点按时点快照名(远端档标题;nil/本机 = 单段标题)。
    case drill(Favorite, deviceId: String, deviceName: String?, path: String)

    var root: AuthorizedRoot {
        switch self {
        case .root(let root): root
        case .sub(let root, _): root
        // 混合页/成员档/钻取档不经本投影(destination 按 case 分派;值恒
        // 不被消费)
        case .fav, .member, .drill: AuthorizedRoot(name: "", path: "", isInbox: false)
        }
    }

    /// 子目录整页直入路径(.root = nil,开页走 loadRoot;.fav 无初始路径;
    /// .member/.drill 亦无——成员/钻取档浏览根即目标路径,直传
    /// AuthorizedRoot)。
    var initialPath: String? {
        switch self {
        case .root, .fav, .member, .drill: nil
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
/// V3-3 T17 起快捷行区段挂载;返工波 4 §16.3 起快捷行树=成员行,见类型
/// 头部「快捷行树=成员行」注)。
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
/// **快捷树展开态**(§16.3,视图侧薄状态——懒加载纪律与收件箱根行同一
/// 面,无持久状态,全现场派生):fav 级箭头翻转 = ``expandedFavIds``;展开
/// = 成员行现拼(devices 快照判离线成员灰禁,在线成员行即现);成员行箭头
/// = 懒加载该成员目录在其设备上的子目录(乐观展开 + 在飞标记
/// ``loadingKeys``,应答落地写 ``memberChildren``,失败收回展开态可再点
/// 重试;子目录行递归同机制);缓存键 device\0path 与根档同空间;收起按
/// 引用释放(同收件箱 collapseAll 语义,收起不留复活面)。树行不进收件箱
/// 选择集(非 entry),选择态点按全部无操作;树位无文件行(§16.3)。
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
    /// 快捷树展开态(§16.3,语义见类型头部「快捷树展开态」注):
    /// 已展开的快捷条目(fav.id;fav 箭头翻转态,收起 = 成员子树整片收拢)。
    @State private var expandedFavIds: Set<String> = []
    /// 已展开的树节点(成员节点 + 子目录节点;键 = 行链身份 favId\0成员
    /// 位次\0device\0path 起,下级续接 device\0path;跨快捷同目录各自独立
    /// 展开态)。乐观展开:点按即入集,应答失败收回(可再点重试)。
    @State private var expandedNodes: Set<String> = []
    /// 该设备某路径子级全量缓存(键 = device\0path;成员节点与树内目录
    /// 子级同一缓存空间,跨快捷/跨节点共享一份,一次展开一次拉取,收起
    /// 按引用释放;缓存存全量条目,渲染层滤隐藏+仅目录——§16.3 树无文件行,
    /// Windows X3 同裁定)。
    @State private var memberChildren: [String: [DirEntry]] = [:]
    /// 路径拉取在飞(键 = device\0path;防重复,在飞点按吞掉)。
    @State private var loadingKeys: Set<String> = []
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
    /// 设备快照(快捷树成员行离线判定 + 设备名 join 数据源,§16.3;进页/
    /// 下拉刷新现拉,失败保旧值——快照未及窗/失败窗恒按离线保守呈现,刷新
    /// 归位)。
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
        // 已展成员/节点键现拉刷新(§16.3;失败保旧值,行不消失);非结构化
        // Task 承载解绑 SwiftUI 取消(R5a #4,三面同款)
        .refreshable {
            await Task {
                await model.refreshStatus()
                await vm.reload()
                await loadDevices()
                await model.favorites.refresh(repo: repo)
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
        // 快捷整单变更(管理屏编辑/他端写回,T17 面单源):已展开条目的
        // 行面随派生即达(成员行 = members 现拼,新成员即现);已删除条目的
        // 展开态与缓存引用一并清理(§16.3 fav 变更纪律)。
        .onChange(of: favorites.favorites) { _, newValue in
            pruneStaleFavorites(Set(newValue.map(\.id)))
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

    // MARK: 快捷行树(V3-3 返工波 4 §16.3:收件箱根行下方 = 快捷条目行;
    //        箭头 = 行内展开成员行(「设备名 - 目录名」,离线灰禁,§13.8
    //        形态回归),行点按 = 推 .fav 混合页;左滑 = [管理, 删除快捷];
    //        选择态无点按无滑面——树行均非收件箱 entry,不进选择集)

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

    /// 快捷条目行:箭头 = 行内展开成员行区(§16.3;有成员才可展开,空成员
    /// = 隐形占位对齐行名——BrowseEntryRow 文件行手法,不示死钮);点行 =
    /// 推 .fav 混合页(选择态无点按);左滑 = [管理, 删除快捷](readOnly
    /// 翻空集——前向守卫禁编辑,管理/删除入口一并收)。
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

    // MARK: 快捷树成员行区(§16.3:快捷行展开 = 成员行——每在线成员一行
    //        「设备名 - 目录名」,离线灰禁;成员行/子目录行箭头 = 懒加载
    //        子目录[仅目录,递归];点行 = 推 .member 整页浏览)

    /// 快捷树区(快捷行展开子级):零成员 = 引导占位行(R4 披露③ 保留,
    /// Windows 零成员展开占位同象);否则每成员一行,展开态成员节点挂
    /// 子目录行区。
    @ViewBuilder private func favTreeArea(_ fav: Favorite) -> some View {
        if fav.members.isEmpty {
            Text("新建快捷目录,再把设备里的目录添加进来")
                .font(.system(size: 13))
                .foregroundStyle(Color(uiColor: .systemGray))
                .padding(.leading, 20 + BrowseViewModel.indentPerLevel)
                .padding(.trailing, 20)
                .frame(height: 44)
                .accessibilityElement(children: .combine)
        } else {
            ForEach(Array(fav.members.enumerated()), id: \.offset) { index, member in
                memberNodeArea(fav, index: index, member: member)
            }
        }
    }

    /// 成员节点区(§16.3):成员行 + 展开态子目录行区。行链身份 = favId\0
    /// 成员位次\0device\0path(重复成员/同路径跨成员身份必不同;下级续接
    /// device\0path,渐进到达序稳定)。
    @ViewBuilder private func memberNodeArea(
        _ fav: Favorite, index: Int, member: FavMember,
    ) -> some View {
        let nodeId = [fav.id, String(index), member.device, member.path]
            .joined(separator: "\u{0000}")
        memberRow(fav, nodeId: nodeId, member: member, online: isMemberOnline(member))
        if expandedNodes.contains(nodeId) {
            memberChildrenArea(
                fav, nodeId: nodeId, device: member.device, path: member.path, depth: 2)
        }
    }

    /// 成员行:caret + 文件夹图标 + 「设备名 - 目录名」(§16.3 词面;设备名
    /// = 快照 join 未及退化 id,目录名 = 路径末段显示名)。**离线成员行灰显
    /// 禁点禁展开**(快照缺该设备按离线;成员行自身灰显即披露,§14.1 注记
    /// 行废止);在线 = 点行 = 推 .member 整页浏览(member = 行目标重建,
    /// 页面数据面只取 device+path),箭头 = 展开/收起(懒加载子目录)。
    private func memberRow(
        _ fav: Favorite, nodeId: String, member: FavMember, online: Bool,
    ) -> some View {
        let name = deviceName(member.device)
        let tail = BrowseFormat.displayName(ofPath: member.path)
        let expanded = expandedNodes.contains(nodeId)
        return Button {
            // 离线禁点(灰显即披露);选择态点按 = 无操作(树行非收件箱
            // entry 不进选择集,根行同款 R5b #1 语义)
            guard online, !vm.isSelecting else { return }
            onOpenMember(fav, member, member.path)
        } label: {
            FullBleedRow(separatorLeadingInset: 20 + BrowseViewModel.indentPerLevel) {
                HStack(spacing: 12) {
                    if online {
                        Button {
                            toggleDirNode(
                                favId: fav.id, nodeId: nodeId, parentRowId: nil,
                                device: member.device, path: member.path)
                        } label: {
                            Image(systemName: "chevron.right")
                                .font(.system(size: 14, weight: .semibold))
                                .foregroundStyle(Color.accentColor)
                                .frame(width: 12)
                                .rotationEffect(.degrees(expanded ? 90 : 0))
                                .padding(8)
                                .contentShape(Rectangle())
                                .padding(.horizontal, -8)
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel(Text(expanded ? "收起" : "展开"))
                    } else {
                        // 离线:12pt 隐形占位(行名左缘对齐不漂,BrowseEntryRow
                        // 文件行手法)
                        Image(systemName: "chevron.right")
                            .font(.system(size: 14, weight: .semibold))
                            .frame(width: 12)
                            .opacity(0)
                    }
                    FileTypeIcon(isDir: true)
                    HStack(spacing: 4) {
                        Text(name)
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundStyle(
                                online ? Color.primary : Color(uiColor: .systemGray))
                            .lineLimit(1)
                        Text(" - \(tail)")
                            .font(.system(size: 12.5))
                            .foregroundStyle(Color(uiColor: .systemGray))
                            .lineLimit(1)
                    }
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 20)
                .padding(.leading, BrowseViewModel.indentPerLevel)
            }
            // 整行触区(R3 真机轮 #1/#5 同款:透明区可点)
            .contentShape(Rectangle())
        }
        .buttonStyle(PressableRowStyle())
        .accessibilityLabel(Text("\(name) - \(tail)" + (online ? "" : ",离线")))
    }

    /// 成员节点子目录行区(懒加载):缓存命中 = 仅目录行(§16.3 树无文件行,
    /// Windows X3 同裁定——缓存存全量,渲染层滤隐藏+仅目录;空目录 = 「空
    /// 文件夹」注记行,Windows emptyDir 同词);未缓存在飞 = 转圈行;不在飞
    /// 无缓存不出(失败已收展开态,行不存在)。
    @ViewBuilder private func memberChildrenArea(
        _ fav: Favorite, nodeId: String, device: String, path: String, depth: Int,
    ) -> some View {
        let key = device + "\u{0000}" + path
        if let children = memberChildren[key] {
            let dirs = BrowseViewModel.sorted(
                children
                    .filter { showHidden || !$0.name.hasPrefix(".") }
                    .filter(\.isDir),
                by: .name)
            if dirs.isEmpty {
                Text("空文件夹")
                    .font(.system(size: 13))
                    .foregroundStyle(Color(uiColor: .systemGray))
                    .padding(.leading, 20 + BrowseViewModel.indentPerLevel * CGFloat(depth))
                    .padding(.trailing, 20)
                    .frame(height: 44)
                    .accessibilityElement(children: .combine)
            } else {
                ForEach(dirs, id: \.path) { entry in
                    childDirRow(fav, parentId: nodeId, device: device, entry: entry, depth: depth)
                }
            }
        } else if loadingKeys.contains(key) {
            favTreeSpinnerRow(depth)
        }
    }

    /// 子目录行(BrowseEntryRow 复用):点行 = 推 .member 整页浏览;箭头 =
    /// 行内展开该目录在其设备上的子目录(递归;仅目录)。
    private func childDirRow(
        _ fav: Favorite, parentId: String, device: String, entry: DirEntry, depth: Int,
    ) -> some View {
        let childId = parentId + "\u{0000}" + device + "\u{0000}" + entry.path
        return BrowseEntryRow(
            entry: entry,
            depth: depth,
            expanded: expandedNodes.contains(childId),
            onOpen: {
                guard !vm.isSelecting else { return }
                onOpenMember(fav, FavMember(device: device, path: entry.path), entry.path)
            },
            onToggleExpand: {
                toggleDirNode(
                    favId: fav.id, nodeId: childId, parentRowId: parentId,
                    device: device, path: entry.path)
            },
        )
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

    // MARK: 快捷树展开态(§16.3;机制镜像收件箱根行与 ``BrowseViewModel``
    //        展开口:懒加载缓存、乐观展开失败收回、在飞吞点、offline 禁展、
    //        收起按引用释放)

    /// 成员键(device + NUL + path;与 store memberKey 同象)。
    private func memberKey(_ member: FavMember) -> String {
        member.device + "\u{0000}" + member.path
    }

    /// 设备显示名(快照 join;未及退化 id——§16.3「设备名 - 目录名」前半)。
    private func deviceName(_ device: String) -> String {
        devices.first { $0.deviceId == device }?.name ?? device
    }

    /// 成员在线判定(devices 快照 join;快照查无此设备 = 离线;快照未及窗
    /// 恒保守离线,刷新/重进页归位)。
    private func isMemberOnline(_ member: FavMember) -> Bool {
        devices.contains { $0.deviceId == member.device && $0.online }
    }

    /// 快捷行箭头点按:展开 = 成员行现拼(§16.3——成员行零预拉,行即现;
    /// 子目录懒加载归成员行箭头);收起 = 成员子树整片收拢 + 缓存按引用
    /// 释放(同收件箱 collapseAll 语义——收起不留复活面;跨快捷共享仍引用
    /// 则留)。
    private func toggleFavoriteExpand(_ fav: Favorite) {
        if expandedFavIds.contains(fav.id) {
            collapseFavorite(fav)
        } else {
            expandedFavIds.insert(fav.id)
        }
    }

    /// 树节点箭头点按(成员节点/子目录节点同口;镜像 ``BrowseViewModel/
    /// toggleExpand``):展开 = 乐观入集(缓存命中即时子行;未缓存即拉,
    /// 应答落地写缓存,失败收回展开态——行仍在可再点重试,错误不打断首页
    /// 主面);在飞点按吞掉(缓存未到不重复拉);收起 = 出集 + 缓存按引用
    /// 释放。
    private func toggleDirNode(
        favId: String, nodeId: String, parentRowId: String?, device: String, path: String,
    ) {
        let key = device + "\u{0000}" + path
        if expandedNodes.contains(nodeId) {
            expandedNodes.remove(nodeId)
            releaseCacheKeyIfUnreferenced(key)
            return
        }
        expandedNodes.insert(nodeId) // 乐观展开(应答失败收回,可再点重试)
        guard memberChildren[key] == nil, !loadingKeys.contains(key) else { return }
        loadingKeys.insert(key)
        spawnListingFetch(
            favId: favId, device: device, path: path,
            nodeId: nodeId, parentRowId: parentRowId)
    }

    /// 单路径拉取并写缓存(成员节点/子目录节点共用)。应答落地守卫(S6
    /// 修复轮,S6 评审 Critical 钉):须 fav 仍展开(收起在飞 = 弃,缓存不
    /// 写不复活);且**父链仍在展开**:nil 父链 = 成员直挂根级,合法;非
    /// nil 父链须在 ``expandedNodes``,断链不保留展开态(R3 评审 I-1 同旨;
    /// 失败 = 收回乐观展开,行仍在可再点重试)。限时 30s(浏览页/混合页
    /// 成员拉取同值口径)。
    private func spawnListingFetch(
        favId: String, device: String, path: String, nodeId: String, parentRowId: String?,
    ) {
        let key = device + "\u{0000}" + path
        Task {
            defer { loadingKeys.remove(key) }
            let result = try? await RpcGuard.withTimeout(seconds: 30) { [repo] in
                try await repo.list(device: device, path: path)
            }
            guard expandedFavIds.contains(favId) else {
                expandedNodes.remove(nodeId) // fav 已收起:展开态不复活
                return
            }
            guard let entries = result?.entries else {
                expandedNodes.remove(nodeId) // 失败:收回乐观展开(行仍在可再点)
                return
            }
            memberChildren[key] = entries
            if let parentRowId, !expandedNodes.contains(parentRowId) {
                expandedNodes.remove(nodeId) // 父链收起在飞:子链不复活
            }
        }
    }

    /// 收起快捷(成员子树整片收拢):本 fav 节点出集 + 本 fav 引用键(成员
    /// 根键 + 已展节点缓存键)按引用计数释放(其他已展开快捷仍引用则留)。
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

    /// 已删除快捷的残留展开态清理(整单变更收口):条目展开态随 id 移除,
    /// 缓存全量引用计数释放(已删除条目的成员键/节点键不再入引用面,无
    /// 引用即释放)。
    private func pruneStaleFavorites(_ liveIds: Set<String>) {
        let staleIds = expandedFavIds.subtracting(liveIds)
        expandedFavIds.subtract(staleIds)
        for stale in staleIds {
            let prefix = stale + "\u{0000}"
            expandedNodes.subtract(expandedNodes.filter { $0.hasPrefix(prefix) })
        }
        for key in Array(memberChildren.keys) where !referencedCacheKeys().contains(key) {
            memberChildren[key] = nil
        }
    }

    /// 下拉刷新跟随:已展开快捷的**在线成员键**全量现拉——缓存命中也重拉
    /// (刷新语义),失败保旧值(旧数据保底,行不消失);离线成员零请求;
    /// 在飞键去重。子节点键(已展节点的缓存键)一并现拉(成员轮在飞键天然
    /// 跳过);失败不改写缓存(行面零变化)。
    private func refreshFavTree() async {
        var memberKeys: Set<String> = []
        for fav in favorites.favorites where expandedFavIds.contains(fav.id) {
            for member in fav.members {
                let key = memberKey(member)
                memberKeys.insert(key)
                guard isMemberOnline(member), !loadingKeys.contains(key) else { continue }
                await refreshListing(device: member.device, path: member.path)
            }
        }
        for key in referencedCacheKeys() where !memberKeys.contains(key) {
            guard !loadingKeys.contains(key) else { continue } // 成员轮在飞:跳过
            let parts = key.split(separator: "\u{0000}", omittingEmptySubsequences: false).map(String.init)
            guard parts.count == 2 else { continue }
            await refreshListing(device: parts[0], path: parts[1])
        }
    }

    /// 单键现拉刷新(缓存命中也拉;失败保旧值——缓存不写不释放,行面零变化)。
    private func refreshListing(device: String, path: String) async {
        let key = device + "\u{0000}" + path
        let result = try? await RpcGuard.withTimeout(seconds: 30) { [repo] in
            try await repo.list(device: device, path: path)
        }
        if let entries = result?.entries {
            memberChildren[key] = entries
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

/// 快捷树扁平行(视图内投影载体;§16.3 成员行树)载体已撤——成员行/子目录
/// 行身份由行链 id(favId\0成员位次\0device\0path 起)现场拼装,无独立载体
/// 结构(§16.3 重写轮)。

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
