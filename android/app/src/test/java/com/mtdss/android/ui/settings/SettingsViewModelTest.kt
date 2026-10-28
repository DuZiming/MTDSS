package com.mtdss.android.ui.settings

import com.mtdss.android.inbox.FakeDocumentsFace
import com.mtdss.android.inbox.InboxGrantStore
import com.mtdss.android.inbox.MemInboxGrantPersistence
import com.mtdss.android.ipc.ConnState
import com.mtdss.android.test.TestKit.awaitUntil
import com.mtdss.android.test.TestKit.runTest
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
import org.junit.Assert.assertTrue
import org.junit.Test
import java.nio.file.Files

/**
 * 设置页 VM JVM 单测:账号信息是会话状态机的投影(登录/在线翻转全由
 * SessionViewModel 驱动,本 VM 零自有状态);更新面整体委托 app 级
 * UpdateController(未配置 fail-closed 的透传面);收件箱面 = SAF 授权
 * 状态投影 + grantInbox 登记(P4a-r2)。
 */
class SettingsViewModelTest {

    @Test
    fun `账号信息投影自会话状态`() = runTest {
        val session = MutableStateFlow(SessionViewModel.State(conn = ConnState.Connected))
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        try {
            val vm = SettingsViewModel(session, "0.1.0", 3, controller(scope), grantStore(), scope)
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
            val vm = SettingsViewModel(session, "0.1.0", 3, controller(scope), grantStore(), scope)
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
            val vm = SettingsViewModel(session, "0.1.0", 3, unconfigured, grantStore(), scope)
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
            val vm = SettingsViewModel(session, "0.1.0", 3, controller(scope), store, scope)
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

    /** 共享伪树面(授权登记甄别用)。 */
    private val storeFace = FakeDocumentsFace()

    private fun grantStore(): InboxGrantStore = InboxGrantStore(MemInboxGrantPersistence(), storeFace)

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
