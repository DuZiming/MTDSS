import SwiftUI
import MTDSSKit
import UIKit

@main
struct MTDSSApp: App {
    @Environment(\.scenePhase) private var scenePhase
    @State private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            AppTabView()
                .environment(model)
                // T10 子模型独立注册(各屏按型观察,AppModel 不转发)
                .environment(model.session)
                .environment(model.account)
                .environment(model.trusted)
                .task { await model.bootEngine() }
                // 传输进行中亮屏,终态复位(spec §5.5;Android KEEP_SCREEN_ON 同象)
                .onChange(of: model.transferActive) { _, active in
                    UIApplication.shared.isIdleTimerDisabled = active
                }
                // 前台回归重对齐(T6-minor3 携带):种子→订阅竞窗内错过的
                // 事件以 transfer.list 快照收口(亮屏面校正;通知去重集对账)
                .onChange(of: scenePhase) { _, phase in
                    if phase == .active {
                        Task { await model.realignOnActive() }
                    }
                }
        }
    }
}

/// 应用级模型(T4 最小面 + T5 status 投影 + T6 传输活动面 + T10 会话/信任
/// 面):引擎 boot 就绪即持有 Repository;快捷目录根数据源(recv_dir/self
/// device id)经 ``refreshStatus`` 投影;传输活动 ``transferActive`` 事件
/// 驱动(spec §5.5 亮屏面,AppTabView 据此翻转 isIdleTimerDisabled)。
/// T10:会话(SessionModel)、账号档案(AccountStore)、信任目录账本
/// (TrustedRootsStore)以子模型供给(各屏经环境独立观察);信任目录增删
/// = 账本 + engine-roots.conf 重写 + 引擎重启(Ruling L,guard 构造期
/// 定死,重启 ~1-2s,UI 显「重连中」)。
@MainActor
@Observable
final class AppModel {
    private(set) var repository: MtdssRepository?
    private(set) var bootError: String?

    /// 本机 device_id(status.device;未登录为 null;T10 起登录/登出后
    /// refreshStatus 重投影)。
    private(set) var selfDeviceId: String?
    /// 收件目录(status.recv_dir;快捷目录根「收件箱」条目的路径)。
    private(set) var recvDir: String?

    /// 任一非终态任务在传(spec §5.5:isIdleTimerDisabled 的应置值)。
    /// 事件驱动 + boot 快照种子;Android KEEP_SCREEN_ON 同口径。
    private(set) var transferActive = false
    private let activityTracker = TransferActivityTracker()
    /// 事件扇出(T6 消费传输活动;T9 传输列表另开订阅,同源不抢流)。
    private var eventHub: EventHub?
    /// 传输完成本地通知(spec §3;T9):判定面 Kit 可测
    /// (``TransferCompletionAnnouncer``),UN 绑定在 App 目标
    /// (Support/LocalNotify.swift;首次通知时请求一次授权)。
    private let notifier: any LocalNotifier
    private var completionAnnouncer = TransferCompletionAnnouncer()

    // MARK: T10 子模型(环境注册,各屏独立观察)

    /// 会话状态面(status 刷新 + event.login 翻转 + 登出链)。
    let session = SessionModel()
    /// 账号档案(email/设备名/高级选项;UserDefaults)。
    let account: AccountStore
    /// 信任目录账本(security-scoped 书签;trust-roots.json 在 App Support,
    /// 引擎 home 清空亦不伤)。
    let trusted = TrustedRootsStore(ledgerURL: AppModel.trustLedgerURL())
    /// 分享队列面(T11,spec §4.7/§5.4):扩展入队的转交项消费 + 设备缓存
    /// 写入 + 「分享转交失败」行投影(App Group 容器;引擎 home 不参与)。
    let share = ShareCenter.make()
    /// 快捷目录 store(V3-3 T14,spec §3):账号 KV 整单 LWW 收敛面;登录态
    /// 拉取在 ``refreshStatus``,data_changed 增量在 ``runFavoritesWatcher``
    /// (T15 接线;store 本体不持会话概念)。
    let favorites = FavoritesStore()

    /// 显示隐藏项(spec §4.6,App 侧开关——引擎不参与,daemon list 本就
    /// 不滤 dotfiles;浏览面经 BrowseViewModel.showHidden 输入消费)。
    var showHidden: Bool {
        didSet { defaults.set(showHidden, forKey: Self.showHiddenKey) }
    }

    /// 允许删除远端文件(R9 #3,App 侧安全门;引擎 ctrl delete 契约不变,
    /// 本开关只收 App UI 面——远端浏览行左滑删除/选择底栏删除的显隐。
    /// 默认关 = 今日集;本地删除不设门)。设置页开关,浏览页消费。
    var allowRemoteDelete: Bool {
        didSet { defaults.set(allowRemoteDelete, forKey: Self.allowRemoteDeleteKey) }
    }

    /// 引擎重启进行中(信任目录变更面;UI 显「重连中」)。
    private(set) var engineRestarting = false

    // MARK: V3-1 受信终端审批(event.approval;AppTabView 根部弹窗)

    /// 受信终端登录审批请求(弹窗载荷;应答经 approval.respond)。
    struct ApprovalRequest: Equatable {
        let sessionId: String
        let candidateName: String
        let candidatePlatform: String?
        let code: String
        let expiresAt: Int64
    }

    /// 在场审批请求(nil = 无挂起;登出/应答成功/稍后即清)。
    private(set) var pendingApproval: ApprovalRequest?
    /// 审批应答在飞(弹窗两键置灰防连点)。
    private(set) var approvalResponding = false
    /// 应答失败文案(弹窗内呈现;请求保持可重试,桌面/android 同款语义)。
    private(set) var approvalRespondError: String?

    private let defaults: UserDefaults

    init(notifier: any LocalNotifier = UNLocalNotifier(), defaults: UserDefaults = .standard) {
        self.notifier = notifier
        self.defaults = defaults
        self.account = AccountStore(defaults: defaults)
        #if DEBUG
        // 冒烟钩子:MTDSS_DEBUG_SHOWHIDDEN=1 强制开(T10 显示隐藏 E2E 截图;
        // 直写 defaults 不经 didSet——钩子不得污染持久开关;真机/产线不编译)
        if ProcessInfo.processInfo.environment["MTDSS_DEBUG_SHOWHIDDEN"] == "1" {
            defaults.set(true, forKey: Self.showHiddenKey)
        }
        // R9 #3 截图/UITest 双态钩子:「0」「1」都生效(force-off 钉门关面;
        // 与 showHidden 钩子的单边形态不同——远端删除门要钉两态,持久开关
        // 需显式清;真机/产线不编译)
        if let gate = ProcessInfo.processInfo.environment["MTDSS_DEBUG_ALLOWREMOTEDELETE"] {
            defaults.set(gate == "1", forKey: Self.allowRemoteDeleteKey)
        }
        #endif
        self.showHidden = defaults.bool(forKey: Self.showHiddenKey)
        self.allowRemoteDelete = defaults.bool(forKey: Self.allowRemoteDeleteKey)
        trusted.load()
    }

    private static let showHiddenKey = "mtdss.browse.showHidden"
    private static let allowRemoteDeleteKey = "mtdss.browse.allowRemoteDelete"

    /// 信任账本落点:App Support/trust-roots.json(App 容器,跨引擎 home
    /// 生命周期;模拟器反复清 home 不丢授权)。
    private static func trustLedgerURL() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        try? FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        return base.appendingPathComponent("trust-roots.json")
    }

    /// 引擎进进程(spec §5.1):App 启动即在进程内线程跑引擎主循环,就绪以
    /// hello 通为准(EngineHost 轮询)。幂等:已就绪不重复 boot。boot 前先
    /// 同步信任根 conf(书签账本 → 解析路径 → engine-roots.conf),使引擎
    /// start 注入即含信任目录(Ruling L;重启路径同理)。
    func bootEngine() async {
        guard repository == nil else { return }
        do {
            syncTrustedRootsConf()
            let repo = try await ProductionEngine.bootRepository()
            repository = repo
            eventHub = EventHub(client: repo.ipc)
            await refreshStatus()
            // 会话面种子(T10):boot 快照定登录态——登录动作早于本次进程
            // (如上轮会话)时无 event.login 可喂,权威态取自 status
            await session.refresh(repo: repo)
            // 分享扩展会话同步(R10):登录素材镜像进 App Group 扩展 home,
            // 扩展引擎据此免登录直发
            syncSessionToExtension()            // 收件发布器(T8,spec §3/§5.2):fs.publish → move 暂存件进
            // Documents;fs.query_dest → 收件箱真实行。独立订阅流(传输活动
            // 监视/设备页各占一条,同源不抢流)。
            // 照片桥应答器(R7 Ruling P):fs.photo_list/fs.photo_export →
            // 0x06/0x07(相册/照片列表 + 原始字节导出;权限门在应答方)。
            if let hub = eventHub {
                let events = hub.subscribe()
                let layout = ProductionEngine.layout()
                Task { await InboxPublish.run(layout: layout, events: events) }
                let photoEvents = hub.subscribe()
                Task { await PhotoServe.run(layout: layout, events: photoEvents) }
                // 快捷目录增量(T15,spec §7):event.data_changed{favorites}
                // → 整单重拉(他端编辑秒级跟线;其他 key 忽略)
                let favEvents = hub.subscribe()
                Task { await runFavoritesWatcher(events: favEvents, repo: repo) }
            }
            // 分享队列(T11):设备缓存首写 + 失败行呈现 + 队列消费
            // (登录门在 consumeIfNeeded 内——未登录不动队列,保等待态)
            await refreshDeviceCache(repo: repo)
            share.refreshRows()
            await share.consumeIfNeeded(repository: repo, loggedIn: session.loggedIn)
            #if DEBUG
            await Self.seedShareQueueForDebug(share: share)
            await share.consumeIfNeeded(repository: repo, loggedIn: session.loggedIn)
            #endif
            Task { await runTransferMonitor() }
        } catch {
            bootError = "引擎启动失败:\(error)"
        }
    }

    /// 审批应答(V3-1):approval.respond → 成功清弹层;失败弹层内呈错误,
    /// 请求保持可重试(不应答 = 悬置由服务端 TTL 收口,桌面同款语义)。
    func respondApproval(_ req: ApprovalRequest, allow: Bool) async {
        guard let repo = repository, !approvalResponding else { return }
        approvalResponding = true
        approvalRespondError = nil
        defer { approvalResponding = false }
        do {
            try await RpcGuard.withTimeout(seconds: 30) { [repo] in
                try await repo.approvalRespond(sessionId: req.sessionId, allow: allow)
            }
            pendingApproval = nil
        } catch {
            approvalRespondError = "应答失败:\(LoginModel.userErrorText(for: error))"
        }
    }

    /// 「稍后」:不应答直接收起弹层(悬置会话由服务端 TTL 收口)。
    func dismissApproval() {
        pendingApproval = nil
        approvalRespondError = nil
    }

    /// status 投影(快捷目录根数据源;进入标签时幂等重拉,限时防挂)。
    /// 失败静默保留旧值(引擎横幅另有 bootError 兜底)。
    func refreshStatus() async {
        guard let repo = repository else { return }
        do {
            let status = try await RpcGuard.withTimeout(seconds: 5) { [repo] in
                try await repo.status()
            }
            selfDeviceId = status.device?.deviceId
            recvDir = status.recvDir
            // 快捷目录重拉(T15):登录态才拉;失败静默(store 保旧值,沿本函数风格)
            if status.loggedIn {
                await favorites.refresh(repo: repo)
            }
        } catch {
            // 保留旧值:快捷目录以既有 recv_dir 呈现(首轮为空则空态)
        }
    }

    /// 事件订阅句柄(设备页 event.devices 收敛 T7;传输列表 T9 同源另开,
    /// 不抢流)。引擎未 boot 为 nil。
    func subscribeEvents() -> AsyncStream<IpcEvent>? {
        eventHub?.subscribe()
    }

    /// 传输活动消费环(app 生命周期常驻):boot 快照种子(重启恢复的非终态
    /// 任务同样占亮屏)→ event.transfer 逐帧喂追踪器;终态复位即熄屏。
    /// 同环喂会话翻转(T10:event.login/online → SessionModel)。
    private func runTransferMonitor() async {
        guard let repo = repository, let hub = eventHub else { return }
        if let snapshot = try? await RpcGuard.withTimeout(seconds: 10) { [repo] in
            try await repo.transferList()
        } {
            activityTracker.seed(snapshot.jobs)
            transferActive = activityTracker.isActive
            // boot 快照对账:快照里已 done 的作业记入通知去重集——种子→订阅
            // 竞窗内翻转成 done 的事件即便经迟到通道重放,历史完成也不触发
            // 通知(只通知本次会话内新完成的;前台回归 realignOnActive 同口径)
            completionAnnouncer.reconcile(snapshot.jobs)
        }
        let events = hub.subscribe()
        for await event in events {
            session.consume(event)
            if case .transfer(let p) = event {
                activityTracker.apply(p)
                transferActive = activityTracker.isActive
                // 完成通知(spec §3):首条 done 帧即发(会话内每作业恰一次)
                if let done = completionAnnouncer.announce(p) {
                    await announceCompletion(done)
                }
            }
            // 分享队列(T11):设备事件 → 缓存重写(扩展读);登录翻转 →
            // 扩展会话镜像同步(R10:登录=拷贝,登出=撤镜像)+ 消费触发
            // (冒烟路径:smoke-login 经独立连接登录,广播到本环)
            switch event {
            case .devices(let list):
                share.writeDeviceCache(devices: list, selfDeviceId: selfDeviceId)
            case .login(let loggedIn):
                syncSessionToExtension()
                if loggedIn {
                    await share.consumeIfNeeded(repository: repo, loggedIn: true)
                } else {
                    // 登出清挂起审批弹层(approval.respond 须已登录会话;
                    // android ApprovalSheetHost 同口径)
                    pendingApproval = nil
                    approvalRespondError = nil
                }
            case .approval(let sessionId, let candidateName, let candidatePlatform, let code, let expiresAt):
                // 受信终端审批弹窗(任意已登录屏面期间到达即弹;审批只在前台
                // 生效——无推送,事件随渲染到达)
                if session.loggedIn == true {
                    pendingApproval = ApprovalRequest(
                        sessionId: sessionId,
                        candidateName: candidateName,
                        candidatePlatform: candidatePlatform,
                        code: code,
                        expiresAt: expiresAt,
                    )
                    approvalRespondError = nil
                }
            case .deviceEvicted:
                // 被移除:SessionModel 置旗(consume)后对账刷新(daemon 已清
                // 会话,loggedIn 随 status 翻 false;android SessionViewModel 同款)
                await session.refresh(repo: repo)
            default:
                break
            }
        }
    }

    /// 快捷目录增量消费环(T15,app 生命周期常驻):event.data_changed
    /// {key:"favorites"} → 整单重拉(他端编辑秒级跟线;其他 key 忽略)。
    /// 引擎重启随旧 client EOF 自然收口后由 bootEngine 重挂(InboxPublish 同款)。
    private func runFavoritesWatcher(events: AsyncStream<IpcEvent>, repo: MtdssRepository) async {
        for await event in events {
            if case .dataChanged(let key) = event, key == FavoritesStore.kvKey {
                await favorites.refresh(repo: repo)
            }
        }
    }

    /// 前台回归重对齐(T6-minor3 携带):boot 种子→订阅建立之间存在竞窗,
    /// 窗内翻转的事件会被错过——前台 transfer.list 快照全量重置追踪器校正
    /// transferActive;完成通知去重集对账(快照已 done 的作业不再由迟到
    /// 通道补发通知,历史完成不打扰)。
    func realignOnActive() async {
        guard let repo = repository else { return }
        if let snapshot = try? await RpcGuard.withTimeout(seconds: 10) { [repo] in
            try await repo.transferList()
        } {
            activityTracker.seed(snapshot.jobs)
            transferActive = activityTracker.isActive
            completionAnnouncer.reconcile(snapshot.jobs)
        }
        await session.refresh(repo: repo)
        // 分享队列(T11):缓存重写(扩展「定期写入」的前台半边)+ 消费
        // (扩展 open() 拉起/用户手动打开都落在本口)
        await refreshDeviceCache(repo: repo)
        // R10:前台回归即刷新扩展会话镜像(「前台定期」半边;令牌轮换
        // 常态在此收敛)
        syncSessionToExtension()
        await share.consumeIfNeeded(repository: repo, loggedIn: session.loggedIn)
    }

    /// 会话同步(R10,brief 架构 ②):主 App 引擎 home 登录素材镜像进
    /// App Group 扩展 home(Kit ``EngineSessionSync``,原子;登出/素材
    /// 不全 = 撤镜像)。失败静默——扩展侧在场性检查降级队列,主 App 无感。
    /// App Group 不可得 = 无扩展共享面,同样静默。
    private func syncSessionToExtension() {
        guard let groupRoot = ShareQueueStore.appGroupRoot() else { return }
        _ = EngineSessionSync.sync(
            appHome: ProductionEngine.home(),
            extHome: EngineSessionSync.extensionHomeURL(groupRoot: groupRoot).path)
    }

    /// 设备缓存刷新(devices 现拉 → App Group;失败静默,扩展有置灰兜底)。
    /// R12 ①:本机 id 随载荷写入(扩展缓存路径滤己的源);id 未投影时先
    /// status 补拉(boot 首写/事件先到形,缺载荷 = 扩展侧不滤,尽量带齐)。
    private func refreshDeviceCache(repo: MtdssRepository) async {
        if selfDeviceId == nil {
            await refreshStatus()
        }
        if let directory = try? await RpcGuard.withTimeout(seconds: 10) { [repo] in
            try await repo.devices()
        } {
            share.writeDeviceCache(devices: directory.devices, selfDeviceId: selfDeviceId)
        }
    }

    /// 完成通知文案:任务名经 transfer.list 现拉(事件帧不带 name;会话级
    /// 来件终态后仍在注册表快照内,可考);取不到回退「传输任务」。多文件
    /// 注记总项数(与列表行「· N 项」同语言)。
    private func announceCompletion(_ p: TransferProgress) async {
        var name = ""
        if let repo = repository,
           let list = try? await RpcGuard.withTimeout(seconds: 10) { [repo] in
            try await repo.transferList()
        } {
            name = list.jobs.first { $0.id == p.id }?.name ?? ""
        }
        let base = name.isEmpty ? "传输任务" : name
        let body = p.filesTotal > 1 ? "\(base) · 共 \(p.filesTotal) 项" : base
        await notifier.notify(title: "传输完成", body: body)
    }

    // MARK: T10 信任目录(Ruling L:账本 + conf + 引擎重启)

    /// 添加信任目录(系统目录选择器回调):书签入账 → conf 重写 → 引擎
    /// 重启(授权集生效)。返回错误文案(nil = 成功)。
    @discardableResult
    func addTrustedRoot(url: URL) async -> String? {
        do {
            _ = try trusted.add(url: url)
        } catch {
            return "无法授权该目录:\(error.localizedDescription)"
        }
        await applyTrustedChange()
        return nil
    }

    /// 移除信任目录:出账 → conf 重写 → 引擎重启(守卫随即拒该集)。
    func removeTrustedRoot(_ root: TrustedRoot) async {
        trusted.remove(root)
        await applyTrustedChange()
    }

    /// conf 同步:账本存活根(resolveAll 顺带剔除失效书签)+ 收件箱
    /// (布局派生,与引擎同规则)合并落 engine-roots.conf。
    private func syncTrustedRootsConf() {
        let resolved = trusted.resolveAll().map(\.path)
        let paths = EngineRootsConf.merge(inbox: ProductionEngine.layout().inbox, trusted: resolved)
        do {
            try EngineRootsConf.write(home: ProductionEngine.home(), paths: paths)
        } catch {
            // 落盘失败不阻断 boot(引擎按无 conf 启动 = 仅收件箱;错误横幅
            // 面归信任操作路径)
            bootError = "信任目录同步失败:\(error)"
        }
    }

    private func applyTrustedChange() async {
        syncTrustedRootsConf()
        await restartEngine()
    }

    /// 引擎重启(信任目录变更面):优雅停 → 全套 boot 重走(Repository/
    /// 事件泵/监视环/发布器随旧 client EOF 自然收口后重建);期间 UI 显
    /// 「重连中」(~1-2s;信任变更低频,Ruling L 可接受)。
    func restartEngine() async {
        guard repository != nil, !engineRestarting else { return }
        engineRestarting = true
        defer { engineRestarting = false }
        ProductionEngine.host.stop()
        repository = nil
        eventHub = nil
        await bootEngine()
    }

    // MARK: DEBUG 冒烟钩子(模拟器无分享入口驱动;真机/产线不编译,特此报备)

    /// MTDSS_DEBUG_SHARE_SEED="<源文件绝对路径>|<目标设备id>[:badsize]":
    /// 模拟扩展入队(同 ShareQueueStore 写入面两步拷贝),随后正常消费链
    /// 接管(event.login/前台触发)。":badsize" 变体在拷贝完成后把队列内
    /// 文件截短(盘损形态——写入口的尺寸守卫拒错源,盘损只发生在拷后)→
    /// 消费校验拦下 → 「分享转交失败」失败行路径。T11 sim E2E 专用。
    #if DEBUG
    private static func seedShareQueueForDebug(share: ShareCenter) async {
        guard let spec = ProcessInfo.processInfo.environment["MTDSS_DEBUG_SHARE_SEED"],
              !spec.isEmpty else { return }
        let parts = spec.split(separator: "|", maxSplits: 1).map(String.init)
        guard parts.count == 2, let store = share.store else { return }
        let sourcePath = parts[0]
        let targetParts = parts[1].split(separator: ":", maxSplits: 1).map(String.init)
        let targetId = targetParts[0]
        let truncate = targetParts.count > 1 && targetParts[1] == "badsize"
        let source = URL(fileURLWithPath: sourcePath)
        guard FileManager.default.fileExists(atPath: source.path) else { return }
        let attrs = try? FileManager.default.attributesOfItem(atPath: source.path)
        let actual = (attrs?[.size] as? NSNumber)?.int64Value ?? 0
        let name = source.lastPathComponent
        let file = ShareQueueFile(name: name, size: actual)
        let id = UUID().uuidString
        do {
            try store.beginItem(id: id, targetDeviceId: targetId, files: [file], now: Date())
            try store.copyFile(item: id, file: file, from: source)
            try store.finishItem(id: id, now: Date())
            if truncate {
                let queueFile = store.filesDir(id: id).appendingPathComponent(file.name)
                let data = try Data(contentsOf: queueFile)
                try data.prefix(max(0, data.count - 8)).write(to: queueFile)
            }
        } catch {
            try? store.removeItem(id: id)
        }
    }
    #endif
}
