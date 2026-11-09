package com.mtdss.android.ui.settings

import com.mtdss.android.inbox.InboxGrantStore
import com.mtdss.android.send.RootsStore
import com.mtdss.android.send.StoredRoot
import com.mtdss.android.update.UpdateController
import com.mtdss.android.ui.SessionViewModel
import com.mtdss.android.ui.browse.BrowseGates
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * 设置持久化缝(JVM 单测注入内存实现;实装 = SharedPreferences,Graph 装配)
 * ——沿项目存储选型既有口径(RootsStore 类注释:单键小数据 SharedPreferences,
 * 不引 DataStore)。
 */
interface SettingsPersistence {
    fun getString(key: String): String?
    fun putString(key: String, value: String)
}

/** 设置持久化键面(SharedPreferences "login" 文件;与登录屏 server/email 记忆同档)。 */
object SettingsKeys {
    /** 设置页设备名改名(下次登录生效,spec §4.6)。 */
    const val DEVICE_NAME_OVERRIDE = "device_name_override"
    /** 显示隐藏项(spec §4.5,默认关;消费面接线 T13)。 */
    const val SHOW_HIDDEN = "show_hidden"
    /** 允许删除远端文件(spec §4.2,默认关;消费面接线 T13)。 */
    const val ALLOW_REMOTE_DELETE = "allow_remote_delete"
    /** 照片图库共享(spec §4.3,默认关;开关只持运行时权限请求入口——
     *  应答面以实时权限为准,权限即数据面;消费面 T14)。 */
    const val PHOTO_SHARE = "photo_share"
    /** 登录账号 email(登录屏登录成功记账;我的页账号行投影)。 */
    const val LOGIN_EMAIL = "email"
}

/**
 * 设置/账号面 App 侧档案(iOS AccountStore 同象缩小版):设备名改名(下次
 * 登录生效)+ 双开关持久;email/服务器记忆仍由登录屏既有 SharedPreferences
 * 面承载(键面见 [SettingsKeys])。默认值:override = null、双开关 = false
 * (spec §4.2/§4.5「默认关」;键缺省即 false,只认字面 "true")。
 */
class SettingsPrefs(private val persistence: SettingsPersistence) {

    /** 设置页设备名改名(null = 未改名;下次 login.verify 生效)。 */
    var deviceNameOverride: String? = persistence.getString(SettingsKeys.DEVICE_NAME_OVERRIDE)
        ?.takeIf { it.isNotBlank() }
        private set

    /** 显示隐藏项(默认关;App 侧过滤 dotfiles,消费面接线 T13)。 */
    var showHidden: Boolean = persistence.getString(SettingsKeys.SHOW_HIDDEN) == "true"
        private set

    /** 允许删除远端文件(默认关;远端删除入口门,消费面接线 T13)。 */
    var allowRemoteDelete: Boolean = persistence.getString(SettingsKeys.ALLOW_REMOTE_DELETE) == "true"
        private set

    /** 照片图库共享(默认关;开关只持运行时权限请求入口——应答面以实时
     *  权限为准,权限即数据面;消费面 T14)。 */
    var photoShare: Boolean = persistence.getString(SettingsKeys.PHOTO_SHARE) == "true"
        private set

    /** 设备名改名;空/空白名 = 撤回(持久清空;下次登录回落实名/主机名)。 */
    fun setDeviceNameOverride(name: String?) {
        deviceNameOverride = name?.trim()?.takeIf { it.isNotEmpty() }
        persistence.putString(SettingsKeys.DEVICE_NAME_OVERRIDE, deviceNameOverride ?: "")
    }

    fun setShowHidden(v: Boolean) {
        showHidden = v
        persistence.putString(SettingsKeys.SHOW_HIDDEN, v.toString())
    }

    fun setAllowRemoteDelete(v: Boolean) {
        allowRemoteDelete = v
        persistence.putString(SettingsKeys.ALLOW_REMOTE_DELETE, v.toString())
    }

    /** 照片图库共享翻转(持久;只持请求入口——授权授予/撤销归系统设置与
     *  PhotoResponder 的实时权限门,消费面 T14)。 */
    fun setPhotoShare(v: Boolean) {
        photoShare = v
        persistence.putString(SettingsKeys.PHOTO_SHARE, v.toString())
    }
}

/**
 * 设置页 VM(spec §3-8;账号信息 / 收件箱 / 检查更新 / 关于):账号面是
 * app 级 SessionViewModel 的纯投影(登录/在线翻转全由会话状态机驱动);更新
 * 面整体委托 [update](T12 编排器,app 级——APK 下载不随设置页进出重置);
 * 收件箱面 = SAF 目录授权状态 + 授权入口(P4a-r2,安卓留存面——iOS 无 SAF,
 * 安卓收件须授权树,spec §3-8 词表外保留,记录为安卓有意差异)。
 *
 * V3-4 增面(spec §4.6/§4.2/§4.5/§4.3):设备名改名([SettingsPrefs],下次登录
 * 生效)/ 显示隐藏项 + 允许删除远端文件双开关(默认关,门控单源 [gates]
 * 状态流投影——浏览/快捷目录消费面接线 T13)/ 照片图库共享开关(默认关,
 * 只持运行时权限请求入口——应答面以实时权限为准,T14)/ 信任目录管理 =
 * 既有 [roots] SAF 授权根账本投影与增删(roots.set 引擎同步经
 * RootsStore.onChange → serve/RootsSync 自动推送,T14)。
 */
class SettingsViewModel(
    sessionState: StateFlow<SessionViewModel.State>,
    appVersionName: String,
    appVersionCode: Int,
    val update: UpdateController,
    private val inboxGrant: InboxGrantStore,
    private val roots: RootsStore,
    private val prefs: SettingsPrefs,
    private val scope: CoroutineScope,
) {
    data class UiState(
        val loggedIn: Boolean? = null,
        val deviceId: String? = null,
        /** 登录实名(status 投影;生效名 = 改名 > 实名 > 主机名)。 */
        val deviceName: String? = null,
        /** 设置页设备名改名(下次登录生效,spec §4.6)。 */
        val deviceNameOverride: String? = null,
        val selfOnline: Boolean? = null,
        val appVersion: String = "",
        /** 收件箱 SAF 目录授权(未授权 = 首次收件发布将「保存失败」,引导先授权)。 */
        val inboxAuthorized: Boolean = false,
        /** 显示隐藏项(默认关;门控单源投影见 [gates],T13 消费)。 */
        val showHidden: Boolean = false,
        /** 允许删除远端文件(默认关;门控单源投影见 [gates],T13 消费)。 */
        val allowRemoteDelete: Boolean = false,
        /** 照片图库共享(默认关;开关只持权限请求入口,应答面以实时权限为准,T14)。 */
        val photoShare: Boolean = false,
        /** 信任目录动作错误词面(添加失败等;成功路径清空)。 */
        val trustError: String? = null,
    )

    private val _state = MutableStateFlow(
        UiState(
            appVersion = "$appVersionName ($appVersionCode)",
            inboxAuthorized = inboxGrant.treeUri() != null,
            deviceNameOverride = prefs.deviceNameOverride,
            showHidden = prefs.showHidden,
            allowRemoteDelete = prefs.allowRemoteDelete,
            photoShare = prefs.photoShare,
        ),
    )

    val state: StateFlow<UiState> = _state.asStateFlow()

    /**
     * 门控单源流(T13):双开关 → [BrowseGates] 投影(UiState 派生,
     * distinctUntilChanged 消抖)——浏览页与快捷目录 VM 消费同一流,设置页
     * 翻开关即时换(重组驱动,免重拉);初值 = 持久档(消费面冷启动即门)。
     */
    val gates: StateFlow<BrowseGates> = state
        .map { BrowseGates(showHidden = it.showHidden, allowRemoteDelete = it.allowRemoteDelete) }
        .distinctUntilChanged()
        .stateIn(scope, SharingStarted.Eagerly, BrowseGates(showHidden = prefs.showHidden, allowRemoteDelete = prefs.allowRemoteDelete))

    /** 信任目录账本投影(既有 RootsStore;增删后刷新)。 */
    private val _trustedRoots = MutableStateFlow<List<StoredRoot>>(emptyList())

    val trustedRoots: StateFlow<List<StoredRoot>> = _trustedRoots.asStateFlow()

    init {
        scope.launch {
            sessionState.collect { s ->
                _state.value = _state.value.copy(
                    loggedIn = s.loggedIn,
                    deviceId = s.deviceId,
                    deviceName = s.deviceName,
                    selfOnline = s.selfOnline,
                )
            }
        }
        refreshRoots()
    }

    /**
     * 登记 OPEN_DOCUMENT_TREE 授权结果(账本有效性甄别失败 → false,屏面
     * 提示重选;成功刷新状态)。发布前的收件授权闸即此面——无授权时发布
     * 词面「收件箱未授权:请在 App 内完成目录授权后重试」指向本入口。
     */
    fun grantInbox(treeUri: String): Boolean {
        val granted = inboxGrant.grant(treeUri) != null
        _state.value = _state.value.copy(inboxAuthorized = granted)
        return granted
    }

    /**
     * 设置页设备名改名(下次登录生效,spec §4.6):持久 + 状态面同步;空白
     * 名不收(iOS guard !isEmpty 同构,不动既有改名)。
     */
    fun setDeviceName(name: String) {
        if (name.isBlank()) return
        prefs.setDeviceNameOverride(name)
        _state.value = _state.value.copy(deviceNameOverride = prefs.deviceNameOverride)
    }

    /** 显示隐藏项翻转(持久;门控单源 [gates] 随之翻转,T13 消费面即时换)。 */
    fun setShowHidden(v: Boolean) {
        prefs.setShowHidden(v)
        _state.value = _state.value.copy(showHidden = v)
    }

    /** 允许删除远端文件翻转(持久;门控单源 [gates] 随之翻转,T13 消费面即时换)。 */
    fun setAllowRemoteDelete(v: Boolean) {
        prefs.setAllowRemoteDelete(v)
        _state.value = _state.value.copy(allowRemoteDelete = v)
    }

    /** 照片图库共享翻转(持久;运行时权限请求 UX 在屏面,见 SettingsScreen)。 */
    fun setPhotoShare(v: Boolean) {
        prefs.setPhotoShare(v)
        _state.value = _state.value.copy(photoShare = v)
    }

    /**
     * 添加信任目录(OPEN_DOCUMENT_TREE 选择器回调同码路径;选定即授权 =
     * 既有 RootsStore 账本 + 持久授权)。失败置 [UiState.trustError] 词面。
     * roots.set 引擎同步:RootsStore.onChange → serve/RootsSync 自动推送
     * (授权根增删热生效,T14)——本面只动 App 侧账本。
     */
    fun addTrustedRoot(treeUri: String, displayName: String) {
        scope.launch {
            val added = withContext(Dispatchers.IO) { roots.add(treeUri, displayName) }
            _state.value = _state.value.copy(
                trustError = if (added == null) "该目录不可授权,请重选" else null,
            )
            refreshRoots()
        }
    }

    /** 移除信任目录(连带释放持久授权;账本投影刷新;roots.set 同步同上加)。 */
    fun removeTrustedRoot(rootId: String) {
        scope.launch {
            withContext(Dispatchers.IO) { roots.remove(rootId) }
            refreshRoots()
        }
    }

    /** 清信任目录错误词面(弹层关闭;iOS trustError = nil 同构)。 */
    fun clearTrustError() {
        _state.value = _state.value.copy(trustError = null)
    }

    /** 全量重读授权根账本(IO 线程;SharedPreferences + 无 resolver 读面)。 */
    private fun refreshRoots() {
        scope.launch {
            _trustedRoots.value = withContext(Dispatchers.IO) { roots.roots() }
        }
    }
}
