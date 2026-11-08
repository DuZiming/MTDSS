package com.mtdss.android.ui.settings

import com.mtdss.android.inbox.FakeDocumentsFace
import com.mtdss.android.inbox.InboxGrantStore
import com.mtdss.android.inbox.MemInboxGrantPersistence
import com.mtdss.android.ipc.ConnState
import com.mtdss.android.send.FakeSafFace
import com.mtdss.android.send.RootsStore
import com.mtdss.android.send.RootsPersistence
import com.mtdss.android.test.TestKit.awaitUntil
import com.mtdss.android.test.TestKit.runTest
import com.mtdss.android.ui.browse.BrowseGates
import com.mtdss.android.update.FakeHttpStack
import com.mtdss.android.update.UpdateController
import com.mtdss.android.update.UpdateUiState
import com.mtdss.android.ui.SessionViewModel
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.nio.file.Files

/**
 * 设置页 VM JVM 单测:账号信息是会话状态机的投影(登录/在线翻转全由
 * SessionViewModel 驱动,本 VM 零自有状态);更新面整体委托 app 级
 * UpdateController(未配置 fail-closed 的透传面);收件箱面 = SAF 授权
 * 状态投影 + grantInbox 登记(P4a-r2);V3-4 面:设备名改名持久(下次
 * 登录生效)/双开关(显示隐藏、允许删除远端文件)默认关且持久/信任目录
 * 既有 RootsStore 账本投影与增删(roots.set 引擎同步归 T14)。
 */
class SettingsViewModelTest {

    @Test
    fun `账号信息投影自会话状态`() = runTest {
        val session = MutableStateFlow(SessionViewModel.State(conn = ConnState.Connected))
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        try {
            val vm = vm(session, scope)
            awaitUntil { vm.state.value.appVersion.isNotEmpty() }
            // 冷启动(尚无权威 status):loggedIn=null,版本行恒有
            assertEquals(null, vm.state.value.loggedIn)
            assertEquals("0.1.0 (3)", vm.state.value.appVersion)

            // 登录翻转(event.login → SessionViewModel.refresh 的同一投影源)
            session.value = SessionViewModel.State(
                conn = ConnState.Connected,
                loggedIn = true,
                selfOnline = true,
                deviceId = "dev-mix3",
                deviceName = "MIX 3",
            )
            awaitUntil { vm.state.value.deviceName == "MIX 3" }
            assertEquals("dev-mix3", vm.state.value.deviceId)
            assertEquals(true, vm.state.value.selfOnline)

            // 在线单字段翻转(不触发重拉,镜像 SessionViewModel 的事件语义)
            session.value = session.value.copy(selfOnline = false)
            awaitUntil { vm.state.value.selfOnline == false }
            assertEquals("MIX 3", vm.state.value.deviceName)
        } finally {
            scope.cancel()
        }
    }

    @Test
    fun `登出后账号面清空`() = runTest {
        val session = MutableStateFlow(
            SessionViewModel.State(
                conn = ConnState.Connected,
                loggedIn = true,
                selfOnline = false,
                deviceId = "dev-mix3",
                deviceName = "MIX 3",
            ),
        )
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        try {
            val vm = vm(session, scope)
            awaitUntil { vm.state.value.deviceName == "MIX 3" }
            session.value = SessionViewModel.State(conn = ConnState.Connected, loggedIn = false)
            awaitUntil { vm.state.value.loggedIn == false }
            assertEquals(null, vm.state.value.deviceId)
            assertEquals(null, vm.state.value.deviceName)
        } finally {
            scope.cancel()
        }
    }

    @Test
    fun `更新面委托 UpdateController 且未配置透传`() = runTest {
        val session = MutableStateFlow(SessionViewModel.State())
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        try {
            val unconfigured = controller(scope)
            val vm = SettingsViewModel(
                session, "0.1.0", 3, unconfigured, grantStore(), rootsStore(), SettingsPrefs(MemSettingsPersistence()), scope,
            )
            awaitUntil { vm.state.value.appVersion.isNotEmpty() }
            assertEquals(unconfigured, vm.update)
            assertEquals(UpdateUiState.Unconfigured, vm.update.state.value)
        } finally {
            scope.cancel()
        }
    }

    @Test
    fun `收件箱授权状态投影与授权登记翻转`() = runTest {
        val session = MutableStateFlow(SessionViewModel.State())
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        try {
            val store = grantStore()
            val vm = vm(session, scope, grant = store)
            awaitUntil { vm.state.value.appVersion.isNotEmpty() }
            // 冷启动未授权:发布将「保存失败」,状态行如实呈现
            assertFalse(vm.state.value.inboxAuthorized)
            // 授权登记成功(选择器件结果)→ 状态翻转
            assertTrue(vm.grantInbox(storeFace.treeUri))
            assertTrue(vm.state.value.inboxAuthorized)
            // 账号投影不受收件箱面影响
            assertEquals(null, vm.state.value.loggedIn)
        } finally {
            scope.cancel()
        }
    }

    // ── V3-4 面(spec §4.6/§4.2/§4.5;设置页双门 + 设备名迁移)─────────────

    @Test
    fun `双开关默认关且翻转持久`() = runTest {
        val session = MutableStateFlow(SessionViewModel.State())
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        try {
            val store = MemSettingsPersistence()
            val vm = vm(session, scope, account = SettingsPrefs(store))
            awaitUntil { vm.state.value.appVersion.isNotEmpty() }
            // 默认全关(spec §4.2/§4.5「默认关」)
            assertFalse(vm.state.value.showHidden)
            assertFalse(vm.state.value.allowRemoteDelete)
            // 翻转即持久(键面写入) + 状态面同步
            vm.setShowHidden(true)
            vm.setAllowRemoteDelete(true)
            awaitUntil { vm.state.value.showHidden }
            assertTrue(vm.state.value.allowRemoteDelete)
            assertEquals("true", store.map[SettingsKeys.SHOW_HIDDEN])
            assertEquals("true", store.map[SettingsKeys.ALLOW_REMOTE_DELETE])
            // 持久面:VM 重建(下次进程/消费面 T13 读取)仍为开
            val vm2 = vm(session, scope, account = SettingsPrefs(store))
            assertTrue(vm2.state.value.showHidden)
            assertTrue(vm2.state.value.allowRemoteDelete)
        } finally {
            scope.cancel()
        }
    }

    @Test
    fun `门控单源投影 双开关状态流即时换`() = runTest {
        val session = MutableStateFlow(SessionViewModel.State())
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        try {
            val store = MemSettingsPersistence()
            val vm = vm(session, scope, account = SettingsPrefs(store))
            awaitUntil { vm.state.value.appVersion.isNotEmpty() }
            // 默认全关(spec §4.2/§4.5;BrowseGates.DEFAULT 同形)
            assertEquals(BrowseGates.DEFAULT, vm.gates.value)
            // 设置页翻开关 → 门流即时翻转(浏览/快捷目录两 VM 消费同一流,重组驱动)
            vm.setShowHidden(true)
            awaitUntil { vm.gates.value.showHidden }
            vm.setAllowRemoteDelete(true)
            awaitUntil { vm.gates.value.allowRemoteDelete }
            assertEquals(BrowseGates(showHidden = true, allowRemoteDelete = true), vm.gates.value)
            // 持久面:VM 重建(消费面新进程)门流初值 = 持久档
            val vm2 = vm(session, scope, account = SettingsPrefs(store))
            assertEquals(BrowseGates(showHidden = true, allowRemoteDelete = true), vm2.gates.value)
        } finally {
            scope.cancel()
        }
    }

    @Test
    fun `设备名改名持久化且空名不收`() = runTest {
        val session = MutableStateFlow(SessionViewModel.State())
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        try {
            val store = MemSettingsPersistence()
            val vm = vm(session, scope, account = SettingsPrefs(store))
            awaitUntil { vm.state.value.appVersion.isNotEmpty() }
            assertNull(vm.state.value.deviceNameOverride)
            vm.setDeviceName("新名字")
            awaitUntil { vm.state.value.deviceNameOverride == "新名字" }
            assertEquals("新名字", store.map[SettingsKeys.DEVICE_NAME_OVERRIDE])
            // 空白名忽略(iOS guard !name.isEmpty 同构)
            vm.setDeviceName("   ")
            assertEquals("新名字", vm.state.value.deviceNameOverride)
            // 持久面:VM 重建(下次登录的屏面读取)仍见改名
            val vm2 = vm(session, scope, account = SettingsPrefs(store))
            assertEquals("新名字", vm2.state.value.deviceNameOverride)
        } finally {
            scope.cancel()
        }
    }

    @Test
    fun `信任目录账本投影与增删`() = runTest {
        val session = MutableStateFlow(SessionViewModel.State())
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        try {
            val saf = FakeSafFace().apply { treeDocIds["content://tree/1"] = "primary:Download/MTDSS" }
            val vm = vm(session, scope, roots = RootsStore(MemRootsPersistence(), saf))
            awaitUntil { vm.state.value.appVersion.isNotEmpty() }
            assertTrue(vm.trustedRoots.value.isEmpty())
            // 添加(系统选择器回调同码路径)→ 账本投影 + 无错误
            vm.addTrustedRoot("content://tree/1", "MTDSS")
            awaitUntil { vm.trustedRoots.value.size == 1 }
            assertEquals("MTDSS", vm.trustedRoots.value[0].displayName)
            assertEquals(null, vm.state.value.trustError)
            // 移除 → 账本投影清空
            vm.removeTrustedRoot(vm.trustedRoots.value[0].rootId)
            awaitUntil { vm.trustedRoots.value.isEmpty() }
            // 不可授权树(treeDocumentId 缺)→ 错误词面
            vm.addTrustedRoot("content://tree/none", "x")
            awaitUntil { vm.state.value.trustError != null }
            assertNotNull(vm.state.value.trustError)
        } finally {
            scope.cancel()
        }
    }

    /** 共享伪树面(授权登记甄别用)。 */
    private val storeFace = FakeDocumentsFace()

    private fun grantStore(): InboxGrantStore = InboxGrantStore(MemInboxGrantPersistence(), storeFace)

    /** 设置 VM 便捷构造(默认空持久 + 空账本;各测试按需覆写)。 */
    private fun vm(
        session: MutableStateFlow<SessionViewModel.State>,
        scope: CoroutineScope,
        grant: InboxGrantStore = grantStore(),
        roots: RootsStore = rootsStore(),
        account: SettingsPrefs = SettingsPrefs(MemSettingsPersistence()),
    ): SettingsViewModel = SettingsViewModel(
        session, "0.1.0", 3, controller(scope), grant, roots, account, scope,
    )

    /** 空授权根账本(FakeSafFace 查表空)。 */
    private fun rootsStore(): RootsStore = RootsStore(MemRootsPersistence(), FakeSafFace())

    /** 内存设置持久(键值查表;SettingsPersistence 缝的测试实现)。 */
    private class MemSettingsPersistence : SettingsPersistence {
        val map = HashMap<String, String>()
        override fun getString(key: String): String? = map[key]
        override fun putString(key: String, value: String) {
            map[key] = value
        }
    }

    /** 内存授权根持久(RootsPersistence 缝的测试实现)。 */
    private class MemRootsPersistence : RootsPersistence {
        var value: String? = null
        override fun load(): String? = value
        override fun save(json: String) {
            value = json
        }
    }

    /** 未配置控制器(空基址):更新面最安全的缺省形态。 */
    private fun controller(scope: CoroutineScope): UpdateController = UpdateController(
        dlBase = "",
        pubkeyHex = "",
        currentVersion = "0.1.0",
        dir = Files.createTempDirectory("mtdss-settings-test").toFile(),
        http = FakeHttpStack(),
        installer = {},
        scope = scope,
    )
}
