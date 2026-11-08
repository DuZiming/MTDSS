package com.mtdss.android

import android.app.Activity
import android.app.Application
import android.content.Context
import android.os.Bundle
import com.mtdss.android.data.MtdssEventHub
import com.mtdss.android.data.MtdssRepository
import com.mtdss.android.engine.MtdssEngine
import com.mtdss.android.fdpass.FdPassChannel
import com.mtdss.android.favorites.FavoritesStore
import com.mtdss.android.favorites.FavoritesWatcher
import com.mtdss.android.inbox.ContentResolverDocumentsFace
import com.mtdss.android.inbox.DocumentsFace
import com.mtdss.android.inbox.InboxGrantStore
import com.mtdss.android.inbox.InboxProvider
import com.mtdss.android.inbox.InboxStore
import com.mtdss.android.inbox.SafInboxStore
import com.mtdss.android.ipc.IpcClient
import com.mtdss.android.serve.MediaStorePhotoLibrary
import com.mtdss.android.serve.PhotoResponder
import com.mtdss.android.serve.RootsSync
import com.mtdss.android.serve.SafExportCopy
import com.mtdss.android.serve.ServeResponder
import com.mtdss.android.serve.photoPermissionGranted
import com.mtdss.android.send.ContentResolverSafFace
import com.mtdss.android.send.RootsStore
import com.mtdss.android.send.SafFace
import com.mtdss.android.send.SourceProvider
import com.mtdss.android.share.ShareFailureController
import com.mtdss.android.share.ShareQueueStore
import com.mtdss.android.ui.SessionViewModel
import com.mtdss.android.ui.settings.SettingsPersistence
import com.mtdss.android.ui.settings.SettingsPrefs
import com.mtdss.android.ui.settings.SettingsViewModel
import com.mtdss.android.ui.tasks.TasksViewModel
import com.mtdss.android.update.UpdateController
import com.mtdss.android.update.UpdateInstaller
import com.mtdss.android.update.UrlConnectionHttpStack
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import java.io.File

/**
 * 应用级装配(单例;MtdssApplication.onCreate 初始化):IpcClient + 事件
 * hub + 仓库 + fdpass 查询通道 + 收件箱发布器 + 发送链(SAF 面/授权根账本/
 * 源应答 Provider)+ 任务面 VM + 更新编排器(T12)+ 会话/设置 VM + 快捷目录
 * store/watcher(T9,构造即订阅事件流),一次构造、进程生命周期共享。
 *
 * **构造序即协议序**:hub 与 InboxProvider/SourceProvider 在此订阅事件流
 * (先于任何屏面动作——收件方向的发布/查询请求与发送方向的 startUpload 都
 * 不经本 App 界面即可触发,fs.publish / fs.query_dest / fs.request_source
 * 的订阅面必须是 app 级);SessionViewModel 随后发起首连(fast-fail,IpcClient
 * 自愈覆盖引擎冷启动间隙)。
 */
object AppGraph {

    @Volatile
    private var instance: Graph? = null

    fun init(context: Context): Graph =
        instance ?: synchronized(this) {
            instance ?: Graph(context).also { instance = it }
        }

    fun get(): Graph = instance ?: error("AppGraph 未初始化(MtdssApplication.onCreate 应先调用 init)")
}

/** 装配产物:屏面只经本对象取依赖,不直接构造。 */
class Graph(context: Context) {

    /** app 级协程域(与进程对齐;屏面级 VM 用各自 remember 域,见屏面)。 */
    val appScope: CoroutineScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    val ipc: IpcClient = IpcClient(MtdssEngine.daemonSocketFile(context), appScope)

    val hub: MtdssEventHub = MtdssEventHub(ipc, appScope)

    val repo: MtdssRepository = MtdssRepository(ipc)

    /**
     * 备案前演练钩子(spec §7 自签 dev CA):{filesDir}/mtdss/dev-ca.pem
     * 候选路径,注入 LoginViewModel;生效闸(DEBUG + 文件存在)在 VM 侧。
     */
    val devCaPath: String = File(MtdssEngine.daemonHome(context), "dev-ca.pem").absolutePath

    /**
     * fdpass 查询通道(收件 0x04 在场查询 + 0x05 发布应答,P4a 起二合一):
     * fd 旁路,socket = {filesDir}/mtdss/fds.sock。daemon 侧 DestQueries 单
     * resolver 统一 id 计数器(spec §6.5),App 单通道单表;0x02 dest 通道已
     * 随收件 fd 桥退役——实例纪律改二(本通道 + 发送源 0x01 通道,见
     * FdPassChannel 类注释)。
     */
    val fdpassQuery: FdPassChannel = FdPassChannel(File(MtdssEngine.daemonHome(context), "fds.sock"), appScope)

    /** SAF 薄面(ContentResolver/DocumentsContract;发送链共用)。 */
    val saf: SafFace = ContentResolverSafFace(context)

    /**
     * 分享暂存队列(V3-4 §4.4 T15):App 私有暂存区 `{filesDir}/share`(
     * **引擎 home 外**——不进授权集、对端不可见;queue/<item>/files 两步
     * 拷贝 + meta.json,见 ShareQueueStore 类注释)。
     */
    val shareQueue: ShareQueueStore = ShareQueueStore(File(context.filesDir, "share"))

    /** 授权根账本(OPEN_DOCUMENT_TREE 持久授权;SharedPreferences 存储)。 */
    val roots: RootsStore = RootsStore(context, saf)

    /**
     * 收件箱 SAF 面(P4a-r2:收件操作整体迁至 SAF 授权树,spec §7)——
     * DocumentsContract 薄面(建目录/建文档/改名/删除/写句柄/扫描器,
     * 与发送链 SafFace 分立:收件须写授权树)。
     */
    val documents: DocumentsFace = ContentResolverDocumentsFace(context)

    /** 收件箱授权账本(ACTION_OPEN_DOCUMENT_TREE 持久授权,读写双旗;独立存储键)。 */
    val inboxGrant: InboxGrantStore = InboxGrantStore(context, documents)

    /** 收件箱存储(SAF 授权树实现;未授权时发布 Failed/查询 false 收口)。 */
    val inboxStore: InboxStore = SafInboxStore(documents, inboxGrant)

    /** 收件箱发布器(构造即订阅 fs.publish / fs.query_dest)。 */
    val inbox: InboxProvider = InboxProvider(ipc, fdpassQuery, appScope, inboxStore)

    /** 发送源应答编排(构造即订阅 fs.request_source;0x01 专用独立通道——同 socket 各自连接,见 SourceProvider 类注释)。分享暂存件(§4.4 T15)同通道应答:stage ref 经注入缝直开 App 私有暂存文件(不进授权集、对端不可见)。 */
    val source: SourceProvider = SourceProvider(
        roots,
        ipc,
        FdPassChannel(File(MtdssEngine.daemonHome(context), "fds.sock"), appScope),
        saf,
        appScope,
        stageFiles = { itemId, name -> shareQueue.openStagedSource(itemId, name) },
    )

    /**
     * 任务面 VM(T11,app 级:KEEP_SCREEN_ON 须在任意屏面期间由活任务驱动,
     * 追踪不随屏面进出重置)。构造即订阅 event.transfer(先于任何屏面动作),
     * 初拉经订阅闸;重发路由经 roots + saf(App 侧重建);分享暂存件重发起
     * 经 shareQueue 在场查询(V3-4 §4.4 T15,清扫窗内有效)。
     */
    val tasks: TasksViewModel = TasksViewModel(
        ipc,
        repo,
        roots,
        saf,
        appScope,
        stageFile = { itemId, name -> shareQueue.stagedFile(itemId, name) },
    )

    /**
     * 检查更新编排器(T12,spec §6;评审裁定①:App 自管,不走 daemon 任务
     * 面):基址/公钥为 BuildConfig 构建期注入(缺省空 = 未配置 fail-closed);
     * 下载落 app 私有 updates/ 目录,进度/重试在设置页呈现。
     */
    val update: UpdateController = UpdateController(
        dlBase = BuildConfig.MTDSS_DL_BASE,
        pubkeyHex = BuildConfig.MTDSS_UPDATE_PUBKEY,
        currentVersion = BuildConfig.VERSION_NAME,
        dir = File(context.filesDir, "updates"),
        http = UrlConnectionHttpStack(),
        installer = { apk -> UpdateInstaller.launch(context, apk) },
        scope = appScope,
    )

    /** 会话状态(app 级 VM,AppNav 路由依据;init 发起首连)。 */
    val session: SessionViewModel = SessionViewModel(repo, hub, appScope)

    /**
     * 设置/账号持久档(SharedPreferences "login"):登录屏 server/email 记忆
     * 与设置面改名/双开关(SettingsKeys 键面)同档——登录/设置共享的账号面。
     */
    private val settingsPrefs = context.getSharedPreferences("login", Context.MODE_PRIVATE)

    /**
     * 快捷目录 store(T9,spec §4.10;iOS FavoritesStore 语义镜像)。app 级
     * 单例 = SWR 缓存本体(重进瞬时/首绘缓存种子;数据_changed 整单重拉);
     * 登出 + 被踢清场归 [favoritesWatcher]。
     */
    val favorites: FavoritesStore = FavoritesStore(repo)

    /**
     * 快捷目录 watcher(T9;iOS T15 接线环镜像,构造即订阅):登录态投影
     * 翻转拉全量/清缓存;event.data_changed{favorites} 增量重拉(他 key
     * 忽略);event.device_evicted 清缓存。devices 快照迟到由 T10 消费面
     * 订阅 hub.devices 重算树。
     */
    val favoritesWatcher: FavoritesWatcher = FavoritesWatcher(
        store = favorites,
        loggedIn = session.state.map { it.loggedIn },
        hub = hub,
        scope = appScope,
    )

    /**
     * 设置页 VM(V3-4 spec §3-8):账号信息 = 会话状态投影;收件箱 = SAF 授权
     * 状态/入口;更新面委托 update;信任目录 = 既有授权根账本(roots.set 引擎
     * 同步经 RootsStore.onChange → RootsSync,T14);设备名改名 + 双开关持久
     * (V3-4)经 settingsPrefs(SharedPreferences "login" 档,与登录屏
     * server/email 记忆同档)。
     */
    val settings: SettingsViewModel = SettingsViewModel(
        sessionState = session.state,
        appVersionName = BuildConfig.VERSION_NAME,
        appVersionCode = BuildConfig.VERSION_CODE,
        update = update,
        inboxGrant = inboxGrant,
        roots = roots,
        prefs = SettingsPrefs(object : SettingsPersistence {
            override fun getString(key: String): String? = settingsPrefs.getString(key, null)
            override fun putString(key: String, value: String) {
                settingsPrefs.edit().putString(key, value).apply()
            }
        }),
        scope = appScope,
    )

    // ── 服务桥(V3-4 §4.1/§4.3,T14:安卓服务桥正身)─────────────────────────

    /**
     * fdpass 服务通道(0x08/0x09/0x0A 单实例——daemon 侧 ServeBridge 单
     * request_id 计数器;与查询/源/照片通道合计四实例,见 FdPassChannel
     * 通道实例纪律)。
     */
    val fdpassServe: FdPassChannel = FdPassChannel(File(MtdssEngine.daemonHome(context), "fds.sock"), appScope)

    /** fdpass 照片通道(0x06/0x07 单实例——daemon 侧 PhotoBridge 单计数器;iOS 契约同号)。 */
    val fdpassPhoto: FdPassChannel = FdPassChannel(File(MtdssEngine.daemonHome(context), "fds.sock"), appScope)

    /**
     * 服务桥应答器(fs.serve_list/export/delete → SAF;构造即订阅 + 挂载
     * 清扫 serve-tmp)。删除缝 = DocumentsFace(App 属主 SAF deleteDocument
     * = 硬删,MIUI 语义库口径);导出拷贝 = SafFace 只读句柄流拷贝等长对账。
     */
    val serveResponder: ServeResponder = ServeResponder(
        saf = saf,
        inboxGrant = { inboxGrant.treeUri() },
        roots = roots,
        serveDir = MtdssEngine.serveTmpDir(context),
        deletion = documents::deleteDocument,
        exportCopy = SafExportCopy(saf),
        channel = fdpassServe,
        ipc = ipc,
        scope = appScope,
    )

    /**
     * 照片桥应答器(fs.photo_list/export → MediaStore;构造即订阅 + 挂载
     * 清扫 photo-serve)。权限即数据面:未授权不应答(引擎侧根隐藏)——
     * 判定实时(每次应答前现查),请求时机 = 设置页/首用,对端浏览不弹窗。
     */
    val photoResponder: PhotoResponder = PhotoResponder(
        photos = MediaStorePhotoLibrary(context),
        serveDir = MtdssEngine.photoServeDir(context),
        channel = fdpassPhoto,
        ipc = ipc,
        scope = appScope,
        authorized = { photoPermissionGranted(context.applicationContext) },
    )

    /**
     * roots.set 引擎同步(T14;T11 双 TODO 的接线本体):启动(连接就绪,
     * 含引擎重启重连补挂)/授权根变更(RootsStore.onChange)/登出清(空
     * 数组)三触发 → repo.rootsSet(收件箱 + 授权根,bridge + 可见)。
     */
    val rootsSync: RootsSync = RootsSync(
        roots = roots,
        push = { specs -> repo.rootsSet(specs) },
        loggedIn = session.state.map { it.loggedIn },
        connState = ipc.connectionState,
        scope = appScope,
    )

    // ── 分享暂存转交(V3-4 §4.4,T15)────────────────────────────────────

    /**
     * 分享转交控制器(传输列表失败行源 + 消费触发;发送闭包 = 桥式
     * startUpload,暂存件作源、App 作 0x01 应答,dir = 收件箱哨兵——与
     * iOS SendService.send 的全平台哨兵同口径):登录翻转/前台(activity
     * resumed,见下)/屏面打开三触发消费;登录前不动队列保等待态。
     */
    val shareFailures: ShareFailureController = ShareFailureController(
        store = shareQueue,
        loggedIn = session.state.map { it.loggedIn }
            .stateIn(appScope, SharingStarted.Eagerly, null),
        scope = appScope,
        send = { items, target ->
            repo.startUpload(items = items, to = target, dir = MtdssRepository.INBOX_DIR)
                .getOrThrow().jobId
        },
    )

    init {
        // 前台消费触发(T15:activity resumed 即视作前台——「打开 App 即
        // 自动发送」的等待件消费面;无 ProcessLifecycleOwner 依赖,零新增)
        (context as? Application)?.registerActivityLifecycleCallbacks(
            object : Application.ActivityLifecycleCallbacks {
                override fun onActivityResumed(activity: Activity) {
                    shareFailures.consumeIfNeeded()
                }

                override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) = Unit
                override fun onActivityStarted(activity: Activity) = Unit
                override fun onActivityPaused(activity: Activity) = Unit
                override fun onActivityStopped(activity: Activity) = Unit
                override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) = Unit
                override fun onActivityDestroyed(activity: Activity) = Unit
            },
        )
    }
}
