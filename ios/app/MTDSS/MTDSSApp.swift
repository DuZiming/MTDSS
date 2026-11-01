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
/// 面):引擎 boot 就绪即持有 Repository;常用目录根数据源(recv_dir/self
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
    /// 收件目录(status.recv_dir;常用目录根「收件箱」条目的路径)。
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

    /// 显示隐藏项(spec §4.6,App 侧开关——引擎不参与,daemon list 本就
    /// 不滤 dotfiles;浏览面经 BrowseViewModel.showHidden 输入消费)。
    var showHidden: Bool {
        didSet { defaults.set(showHidden, forKey: Self.showHiddenKey) }
    }

    /// 引擎重启进行中(信任目录变更面;UI 显「重连中」)。
    private(set) var engineRestarting = false

    private let defaults: UserDefaults

    init(notifier: any LocalNotifier = UNLocalNotifier(), defaults: UserDefaults = .standard) {
        self.notifier = notifier
        self.defaults = defaults
        self.account = AccountStore(defaults: defaults)
        self.showHidden = defaults.bool(forKey: Self.showHiddenKey)
        #if DEBUG
        // 冒烟钩子:MTDSS_DEBUG_SHOWHIDDEN=1 强制开(T10 显示隐藏 E2E 截图;
        // 真机/产线不编译,特此报备)
        if ProcessInfo.processInfo.environment["MTDSS_DEBUG_SHOWHIDDEN"] == "1" {
            showHidden = true
        }
        #endif
        trusted.load()
    }

    private static let showHiddenKey = "mtdss.browse.showHidden"

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
            // 收件发布器(T8,spec §3/§5.2):fs.publish → move 暂存件进
            // Documents;fs.query_dest → 收件箱真实行。独立订阅流(传输活动
            // 监视/设备页各占一条,同源不抢流)。
            if let hub = eventHub {
                let events = hub.subscribe()
                let layout = ProductionEngine.layout()
                Task { await InboxPublish.run(layout: layout, events: events) }
            }
            Task { await runTransferMonitor() }
        } catch {
            bootError = "引擎启动失败:\(error)"
        }
    }

    /// status 投影(常用目录根数据源;进入标签时幂等重拉,限时防挂)。
    /// 失败静默保留旧值(引擎横幅另有 bootError 兜底)。
    func refreshStatus() async {
        guard let repo = repository else { return }
        do {
            let status = try await RpcGuard.withTimeout(seconds: 5) { [repo] in
                try await repo.status()
            }
            selfDeviceId = status.device?.deviceId
            recvDir = status.recvDir
        } catch {
            // 保留旧值:常用目录以既有 recv_dir 呈现(首轮为空则空态)
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
}
