package com.mtdss.android.ipc

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.util.concurrent.CopyOnWriteArrayList

/**
 * IPC 契约 golden 双端锁定:crates/ipc/contract 目录下 *.json(daemon 契约
 * 测试同源,单一真源)逐一对齐 Kotlin 客户端——
 *
 * - `{op}.req.json`:类型化调用经伪 server 走完整客户端路径,线上请求帧与
 *   golden 文件**逐字节**比对(键序 = serde_json 字母序,含尾换行);
 * - `{op}.resp.json`:类型化解析吃下 golden 应答并核对字段值;错误形态核对
 *   RpcException(code/message/kind);
 * - `{event}.json`:事件帧经事件流发射并类型化解析核对;
 * - **孤儿 golden 防护**:目录里出现客户端面未绑定、也未声明不移植的形态
 *   → 立即红(T3 已知盲区:golden 增枚不声不响);全部文件必须被消费。
 *
 * V3-1 增量:登录步进族(verify/submit_code/status/evict/qr_status 五 op
 * 共用 status 判别形态)+ approval.respond/kv.get/kv.put + 三新事件
 * (approval/device_evicted/devices_changed);扫码面仅契约绑定,UI 随 V3-4;
 * Ruling R8 增补:qr.claim/qr.approve 手机侧透传 op(候选面拍平/裁决状态
 * 投影)+ 死码/重放两张变体帧。
 * V3-3 增量:data_changed 事件(key 投影;UI 消费随 V3-4)。
 * V3-4 增量:roots.set 热注册(本波唯一新增客户端 op; RootsSync 启动/变更/
 * 登出三触发推送的线格式由此逐字节锁定)。
 */
class ContractGoldenTest {

    /** 客户端面不移植的 req 形态(spec §4.2 便携面之外;golden 仅解析校验)。
     * V3-4 起 delete/remote_copy 随浏览模板入安卓面(T7)、local_copy 随快捷
     * 目录树行本机源组入安卓面(T10),退出不移植集。 */
    private val notPortedForms = setOf("set_recv_dir")

    /** android 变体 golden(T6 平台投影:status 收件箱哨兵 / set_recv_dir 拒绝)。 */
    private val androidVariants = setOf("status.resp.android.json", "set_recv_dir.resp.android.json")

    /** 形态样本 golden(v2-P5 T5b:list resp 条目形状,children 可选面)。 */
    private val shapeVariants = setOf("list.resp.entries.json")

    /** 登录步进变体 golden(V3-1:verify approval/limit 投影 + qr_status
     * pending;typed 解析在专项用例,forms 只锁主形态)。 */
    private val loginStepVariants = setOf(
        "login.verify.resp.approval.json",
        "login.verify.resp.limit.json",
        "login.qr_status.resp.pending.json",
        "kv.get.resp.notfound.json",
    )

    /** 扫码授权变体 golden(Ruling R8:claim 死码 typed not_found 错误 +
     * approve 单次裁决重放/过期的 expired 投影)。 */
    private val qrApprovalVariants = setOf(
        "qr.claim.resp.expired.json",
        "qr.approve.resp.expired.json",
    )

    /** request_code echo_off 变体 golden(final review C1:产线邮件直投
     * 回 `{}` 无回显,code 容缺省空串)。 */
    private val requestCodeVariants = setOf(
        "login.request_code.resp.echo_off.json",
    )

    /** 事件 golden 文件 → 期望断言。 */
    private val eventChecks: Map<String, (EventFrame) -> Unit> = linkedMapOf(
        "event.login.json" to { f ->
            assertEquals("event.login", f.method)
            val e = MtdssEvent.parse(f) as MtdssEvent.Login
            assertEquals(true, e.loggedIn)
        },
        "event.online.json" to { f ->
            assertEquals("event.online", f.method)
            val e = MtdssEvent.parse(f) as MtdssEvent.Online
            assertEquals(true, e.online)
        },
        "event.devices.json" to { f ->
            assertEquals("event.devices", f.method)
            val e = MtdssEvent.parse(f) as MtdssEvent.Devices
            assertEquals(2, e.devices.size)
            assertEquals("10ba682c8ad13513971e8b56881aab8b", e.devices[0].deviceId)
            assertEquals("契约本机", e.devices[0].name)
            assertEquals("windows", e.devices[0].platform)
            // V3-3 返工 R1:目录行携 created_at(Unix 秒);本机先入账在前
            assertEquals(1730000000L, e.devices[0].createdAt)
            assertEquals("dev-peer-fixed", e.devices[1].deviceId)
            assertEquals("linux", e.devices[1].platform)
            assertEquals(1730086400L, e.devices[1].createdAt)
        },
        "event.transfer.json" to { f ->
            assertEquals("event.transfer", f.method)
            val e = MtdssEvent.parse(f) as MtdssEvent.Transfer
            val p = e.progress
            assertEquals("3", p.id)
            assertEquals("upload", p.kind)
            assertEquals("queued", p.state)
            assertEquals(0L, p.bytesDone)
            assertEquals(4096L, p.bytesTotal)
            assertEquals(0L, p.filesDone)
            assertEquals(1L, p.filesTotal)
            assertEquals(0L, p.skipped)
            assertEquals(0L, p.speedBps)
            assertEquals(false, p.relay)
            assertNull(p.error)
        },
        "event.notify.json" to { f ->
            assertEquals("event.notify", f.method)
            val e = MtdssEvent.parse(f) as MtdssEvent.Notify
            assertEquals("Synshu", e.title)
            assertEquals("«契约对端» 向 /mtdss/contract-inbox 写入 2 个文件", e.body)
        },
        "event.approval.json" to { f ->
            assertEquals("event.approval", f.method)
            val e = MtdssEvent.parse(f) as MtdssEvent.Approval
            assertEquals("sess-contract-a", e.sessionId)
            assertEquals("契约申请端", e.candidateName)
            assertEquals("windows", e.candidatePlatform)
            assertEquals("123456", e.code)
            assertEquals(1760000200L, e.expiresAt)
        },
        "event.device_evicted.json" to { f ->
            assertEquals("event.device_evicted", f.method)
            assertEquals(MtdssEvent.DeviceEvicted, MtdssEvent.parse(f))
        },
        "event.devices_changed.json" to { f ->
            assertEquals("event.devices_changed", f.method)
            assertEquals(MtdssEvent.DevicesChanged, MtdssEvent.parse(f))
        },
        "event.data_changed.json" to { f ->
            assertEquals("event.data_changed", f.method)
            val e = MtdssEvent.parse(f) as MtdssEvent.DataChanged
            assertEquals("favorites", e.key)
        },
    )

    /**
     * 客户端面全形态注册表:golden 形态名 → 类型化调用 + 结果断言。
     * 形态名 = `{op}.req.json` 去后缀(transfer.start 的多形态各占一枚)。
     */
    private val forms: Map<String, suspend IpcClient.() -> Unit> = linkedMapOf(
        "hello" to {
            val v = hello().getOrThrow()
            assertEquals("1.3.0", v.version)
            assertEquals(true, v.loggedIn)
            assertEquals("10ba682c8ad13513971e8b56881aab8b", v.device?.deviceId)
            assertNull(v.device?.name)
        },
        "status" to {
            val v = status().getOrThrow()
            assertEquals(true, v.loggedIn)
            assertEquals(false, v.online)
            assertEquals("http://127.0.0.1:39731", v.server)
            assertEquals("/mtdss/contract-inbox", v.recvDir)
            assertEquals("10ba682c8ad13513971e8b56881aab8b", v.device?.deviceId)
        },
        "devices" to {
            val v = devices().getOrThrow()
            assertEquals(false, v.stale)
            assertEquals(2, v.devices.size)
            assertEquals("10ba682c8ad13513971e8b56881aab8b", v.devices[0].deviceId)
            assertEquals("契约本机", v.devices[0].name)
            assertEquals("windows", v.devices[0].platform)
            assertEquals(false, v.devices[0].online)
            // R1:快照携 created_at(Unix 秒,加入顺序排序键)
            assertEquals(1730000000L, v.devices[0].createdAt)
            assertEquals("dev-peer-fixed", v.devices[1].deviceId)
            assertEquals("契约对端", v.devices[1].name)
            assertEquals("linux", v.devices[1].platform)
            assertEquals(1730086400L, v.devices[1].createdAt)
        },
        "login.request_code" to {
            val v = loginRequestCode("http://127.0.0.1:39731", "contract@example.com").getOrThrow()
            assertEquals("654321", v.code)
        },
        "login.verify" to {
            val ok = loginVerify("654321", "contract@example.com", "契约本机").getOrThrow() as LoginStep.Ok
            assertEquals("10ba682c8ad13513971e8b56881aab8b", ok.deviceId)
            assertEquals("契约本机", ok.name)
        },
        "login.submit_code" to {
            val ok = loginSubmitCode("sess-contract-a", "123456").getOrThrow() as LoginStep.Ok
            assertEquals("10ba682c8ad13513971e8b56881aab8b", ok.deviceId)
            assertEquals("契约本机", ok.name)
        },
        "login.status" to {
            val ok = loginStatus("sess-contract-b").getOrThrow() as LoginStep.Ok
            assertEquals("契约本机", ok.name)
        },
        "login.evict" to {
            val ok = loginEvict("enroll-contract-fixed", "dev-peer-fixed").getOrThrow() as LoginStep.Ok
            assertEquals("契约本机", ok.name)
        },
        "login.cancel" to {
            assertEquals(Unit, loginCancel().getOrThrow())
        },
        "login.qr_begin" to {
            val v = loginQrBegin("http://127.0.0.1:39731", "契约本机").getOrThrow()
            assertEquals("https://www.mtdss.top/qr?t=qrt-contract-fixed", v.qrUrl)
            assertEquals("qrt-contract-fixed", v.token)
            assertEquals(1760000300L, v.expiresAt)
        },
        "login.qr_status" to {
            assertTrue(loginQrStatus("qrt-contract-fixed").getOrThrow() is LoginStep.Ok)
        },
        "approval.respond" to {
            assertEquals(Unit, approvalRespond("sess-contract-a", true).getOrThrow())
        },
        "qr.claim" to {
            val v = qrClaim("qrt-contract-fixed").getOrThrow()
            assertEquals("契约新机", v.candidateName)
            assertEquals("windows", v.candidatePlatform)
            assertEquals(1760000300L, v.expiresAt)
        },
        "qr.approve" to {
            val v = qrApprove("qrt-contract-fixed", true).getOrThrow()
            assertEquals("ok", v.status)
        },
        "kv.get" to {
            val v = kvGet("常用目录").getOrThrow()
            assertEquals("""{"dirs":["/工作"]}""", v.value)
        },
        "kv.put" to {
            assertTrue(kvPut("常用目录", """{"dirs":["/工作"]}""").getOrThrow().ok)
        },
        "roots.set" to {
            // V3-4 §4.1 热注册入锚:请求帧字节锁三生产形态——bridge 可见根
            // (安卓收件箱)、bridge 隐藏根(暂存区 visible=false)、fs 根 + 空
            // 名(Windows 信任模式,空名 = 守卫回退末段目录名);成功面恒 ok
            // (空数组清回直通同响应,无独立形态)。
            assertEquals(Unit, rootsSet(
                listOf(
                    RootSpec("收件箱", "/收件箱", "bridge", true),
                    RootSpec("暂存", "/暂存区", "bridge", false),
                    RootSpec("", "/home/u/资料", "fs", true),
                ),
            ).getOrThrow())
        },
        "login.logout" to {
            assertEquals(Unit, loginLogout().getOrThrow())
        },
        "list" to {
            val e = list("dev-peer-fixed", "/远端/文档").exceptionOrNull() as RpcException
            assertEquals(3L, e.code)
            assertEquals("设备离线: dev-peer-fixed", e.message)
            assertNull(e.kind)
        },
        "stat" to {
            val e = stat("dev-peer-fixed", "/远端/契约 文档.pdf").exceptionOrNull() as RpcException
            assertEquals(3L, e.code)
            assertEquals("设备离线: dev-peer-fixed", e.message)
        },
        "delete" to {
            // golden 应答 = 错误形态(对端离线);请求帧已锁 {device,paths,force}
            val e = delete("dev-peer-fixed", listOf("/远端/契约 文档.pdf"), force = false).exceptionOrNull() as RpcException
            assertEquals(3L, e.code)
            assertEquals("设备离线: dev-peer-fixed", e.message)
        },
        "transfer.start.upload" to {
            val v = transferStartUpload(
                files = listOf("contract-fixture/契约样本.bin"),
                to = "dev-peer-fixed",
                dir = "/golden/peer-inbox",
                skip = emptyList(),
                overwrite = emptyList(),
                cut = false,
            ).getOrThrow()
            assertEquals("3", v.jobId)
        },
        "transfer.start.upload-bridge" to {
            val v = transferStartUploadBridge(
                items = listOf(BridgeItem(ref = "bridge-sample-ref", rel = "契约桥接样本.bin", size = 4096, mtime = 1_760_000_000_000)),
                to = "dev-peer-fixed",
                dir = "/golden/peer-inbox",
                skip = emptyList(),
                overwrite = emptyList(),
                cut = false,
            ).getOrThrow()
            assertEquals("6", v.jobId)
        },
        "transfer.start.download" to {
            val e = transferStartDownload(
                device = "dev-peer-fixed",
                paths = listOf("/远端/契约 文档.pdf"),
                dir = "golden/inbox",
                skip = emptyList(),
                overwrite = emptyList(),
                cut = false,
            ).exceptionOrNull() as RpcException
            assertEquals(1L, e.code)
            assertEquals("dir 非法(须为绝对路径,Task 1 口径): golden/inbox", e.message)
        },
        "transfer.start.download-inbox" to {
            val e = transferStartDownload(
                device = "dev-peer-fixed",
                paths = listOf("/远端/契约 文档.pdf"),
                dir = "inbox", // 收件箱哨兵(T6):同一 builder,dir 取哨兵值
                skip = emptyList(),
                overwrite = emptyList(),
                cut = false,
            ).exceptionOrNull() as RpcException
            assertEquals(1L, e.code)
            assertEquals("dir 非法(须为绝对路径,Task 1 口径): inbox", e.message)
        },
        "transfer.start.remote_copy" to {
            val v = transferStartRemoteCopy(
                from = "dev-peer-fixed",
                paths = listOf("/远端/直传 目录"),
                to = "dev-peer2-fixed",
                dir = "/golden/peer2-inbox",
                skip = emptyList(),
                overwrite = emptyList(),
                cut = false,
            ).getOrThrow()
            assertEquals("10ba682c8ad13513971e8b56881aab8b:5", v.jobId)
        },
        "transfer.start.local_copy" to {
            // golden 应答 = 错误形态(dir 非绝对路径被引擎预检拒);请求帧已锁
            // {kind:"local_copy",paths,dir,cut}(skip/overwrite 空集省键,T10)
            val e = transferStartLocalCopy(
                paths = listOf("contract-fixture/契约样本.bin"),
                dir = "golden/inbox",
            ).exceptionOrNull() as RpcException
            assertEquals(1L, e.code)
            assertEquals("dir 非法(须为绝对路径,Task 1 口径): golden/inbox", e.message)
        },
        "transfer.list" to {
            val v = transferList().getOrThrow()
            assertEquals(3, v.jobs.size)
            val j1 = v.jobs[0]
            assertEquals("1", j1.id)
            assertEquals("upload", j1.kind)
            assertEquals("契约样本.bin", j1.name)
            assertEquals("failed", j1.state)
            assertEquals(0L, j1.filesDone)
            assertEquals(1L, j1.filesTotal)
            assertEquals(0L, j1.bytesDone)
            assertEquals(4096L, j1.bytesTotal)
            assertEquals("contract-fixture/契约样本.bin", j1.src)
            assertEquals("dev-peer-fixed:/golden/peer-inbox", j1.dst)
            assertEquals("发送失败(共尝试 6 轮,最后错误: 信令未在线,无法发起发送)", j1.error)
            val j2 = v.jobs[1]
            assertEquals("2", j2.id)
            assertEquals("download", j2.kind)
            assertEquals("cancelled", j2.state)
            assertEquals("dev-peer-fixed:/远端/契约 文档.pdf", j2.src)
            assertEquals("/golden/inbox", j2.dst)
            assertNull(j2.error)
            val j3 = v.jobs[2]
            assertEquals("3", j3.id)
            assertEquals("cancelled", j3.state)
            assertEquals(0L, j3.speedBps)
            assertEquals(false, j3.relay)
        },
        "transfer.pause" to {
            assertEquals(Unit, transferPause("3").getOrThrow())
        },
        "transfer.resume" to {
            assertEquals(Unit, transferResume("3").getOrThrow())
        },
        "transfer.cancel" to {
            assertEquals(Unit, transferCancel("3").getOrThrow())
        },
        "transfer.wait" to {
            val v = transferWait("3").getOrThrow()
            assertEquals("cancelled", v.state)
            assertNull(v.error)
            assertEquals(0L, v.filesDone)
            assertEquals(1L, v.filesTotal)
            assertEquals(4096L, v.bytesTotal)
            assertEquals(0L, v.skipped)
        },
        "transfer.reinitiate" to {
            assertEquals(Unit, transferReinitiate("1").getOrThrow())
        },
        "transfer.clear" to {
            assertEquals(Unit, transferClear("2").getOrThrow())
        },
    )

    @Test
    fun `全部 req 形态——请求帧逐字节对齐且应答类型化解析吃下 golden`() = runBlocking {
        val dir = Goldens.dir()
        val reqFiles = dir.listFiles { f: File -> f.name.endsWith(".req.json") }!!
            .sortedBy { it.name }
        assertTrue("golden 目录为空", reqFiles.isNotEmpty())

        // 孤儿 golden 防护:未绑定且未声明不移植的新形态 → 红
        val unbound = reqFiles.map { it.name.removeSuffix(".req.json") }.filter { it !in forms && it !in notPortedForms }
        assertTrue("golden 出现未绑定形态(客户端面缺绑定,或需声明不移植): $unbound", unbound.isEmpty())

        for (f in reqFiles) {
            val form = f.name.removeSuffix(".req.json")
            if (form in notPortedForms) continue // 解析校验在「全量文件无遗漏」用例
            val respBytes = dir.resolve("$form.resp.json").readBytes()
            val fake = FakeDaemon()
            fake.onLine = { conn, line ->
                val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
                conn.sendLine(Goldens.withId(respBytes, id))
            }
            val client = IpcClient(fake, CoroutineScope(SupervisorJob()), backoff = { 25L })
            try {
                withTimeout(10_000) { forms.getValue(form)(client) }
                // 字节级请求帧比对:golden 文件内容 = 线上帧(含尾换行)
                assertEquals("$form 恰好发出一个请求", 1, fake.connections.single().received.size)
                assertArrayEquals("$form 请求帧漂移", f.readBytes(), fake.connections.single().received[0])
            } catch (e: Throwable) {
                throw AssertionError("形态 $form 失败: ${e.message}", e)
            } finally {
                client.close()
            }
        }
    }

    @Test
    fun `android 变体——status 收件箱哨兵投影`() = runBlocking {
        val fake = FakeDaemon()
        fake.onLine = { conn, line ->
            val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
            conn.sendLine(Goldens.withId(Goldens.read("status.resp.android.json"), id))
        }
        val client = IpcClient(fake, CoroutineScope(SupervisorJob()), backoff = { 25L })
        try {
            val v = withTimeout(10_000) { client.status().getOrThrow() }
            assertEquals("inbox", v.recvDir) // INBOX_SENTINEL(T6 平台投影)
            assertEquals(true, v.loggedIn)
        } finally {
            client.close()
        }
    }

    @Test
    fun `list 条目形态样本——DirEntry children 可选面(v2-P5 T5b)`() = runBlocking {
        // 成功形态需对端在线(fixture 不可达),线上 JSON 形状由 daemon 构造
        // 帧钉死——目录项携 children(直接子项数),文件项整键缺省 → null。
        val fake = FakeDaemon()
        fake.onLine = { conn, line ->
            val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
            conn.sendLine(Goldens.withId(Goldens.read("list.resp.entries.json"), id))
        }
        val client = IpcClient(fake, CoroutineScope(SupervisorJob()), backoff = { 25L })
        try {
            val v = withTimeout(10_000) { client.list("dev-peer-fixed", "/远端/文档").getOrThrow() }
            assertEquals(false, v.truncated)
            val dir = v.entries[0]
            assertEquals("文档 目录", dir.name)
            assertEquals(true, dir.isDir)
            assertEquals("目录项携直接子项数", 2L, dir.children!!)
            val file = v.entries[1]
            assertEquals("契约 文档.pdf", file.name)
            assertEquals(false, file.isDir)
            assertNull("文件项无 children 键 → null", file.children)
        } finally {
            client.close()
        }
    }

    @Test
    fun `登录步进变体——approval limit pending 投影类型化解析`() = runBlocking {
        // V3-1:verify/qr_status 的非 ok 应答变体(status 判别面中间态),
        // typed 步进逐字段吃下 golden;主形态(req/resp 主对)由 forms 锁。
        val cases: Map<String, suspend (IpcClient) -> LoginStep> = linkedMapOf(
            "login.verify.resp.approval.json" to {
                it.loginVerify("654321", "contract@example.com", "契约本机").getOrThrow()
            },
            "login.verify.resp.limit.json" to {
                it.loginVerify("654321", "contract@example.com", "契约本机").getOrThrow()
            },
            "login.qr_status.resp.pending.json" to {
                it.loginQrStatus("qrt-contract-fixed").getOrThrow()
            },
        )
        for ((name, call) in cases) {
            val fake = FakeDaemon()
            fake.onLine = { conn, line ->
                val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
                conn.sendLine(Goldens.withId(Goldens.read(name), id))
            }
            val client = IpcClient(fake, CoroutineScope(SupervisorJob()), backoff = { 25L })
            try {
                val step = withTimeout(10_000) { call(client) }
                when (name) {
                    "login.verify.resp.approval.json" -> {
                        val a = step as LoginStep.Approval
                        assertEquals("sess-contract-a", a.sessionId)
                        assertEquals(1760000200L, a.expiresAt)
                    }
                    "login.verify.resp.limit.json" -> {
                        val l = step as LoginStep.Limit
                        assertEquals("enroll-contract-fixed", l.enrollToken)
                        assertEquals(2, l.devices.size)
                        assertEquals("契约本机", l.devices[0].name)
                        assertEquals("windows", l.devices[0].platform)
                        assertEquals("契约对端", l.devices[1].name)
                        assertEquals("linux", l.devices[1].platform)
                        // R1:踢选清单面(auth::DeviceInfo)不携 created_at
                        // ——解析容忍落 null,不炸面。
                        assertNull(l.devices[0].createdAt)
                    }
                    "login.qr_status.resp.pending.json" -> {
                        val p = step as LoginStep.Pending
                        assertEquals(1760000300L, p.expiresAt)
                    }
                }
            } catch (e: Throwable) {
                throw AssertionError("登录步进变体 $name 失败: ${e.message}", e)
            } finally {
                client.close()
            }
        }
    }

    @Test
    fun `kv get 未命中形态——typed not_found 错误投影`() = runBlocking {
        val fake = FakeDaemon()
        fake.onLine = { conn, line ->
            val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
            conn.sendLine(Goldens.withId(Goldens.read("kv.get.resp.notfound.json"), id))
        }
        val client = IpcClient(fake, CoroutineScope(SupervisorJob()), backoff = { 25L })
        try {
            val e = withTimeout(10_000) { client.kvGet("不存在键").exceptionOrNull() } as RpcException
            assertEquals(1L, e.code)
            assertEquals("not_found", e.kind)
            assertEquals("无此键: 不存在键", e.message)
        } finally {
            client.close()
        }
    }

    @Test
    fun `devices 快照 created_at 旧 daemon 缺键容忍`() = runBlocking {
        // V3-3 返工 R1 容忍面:旧 daemon 的 devices 响应缺 `created_at` 键
        // → 解析不炸,createdAt 落 null(色板任务对 null 回退 device_id 序,
        // spec §13.4)。携键正形态已由 forms "devices" 锁死,此处锁容忍面。
        val fake = FakeDaemon()
        fake.onLine = { conn, line ->
            val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
            conn.sendLine(
                """{"id":$id,"result":{"devices":[{"device_id":"d1","name":"旧 daemon 条目","""
                    .trimIndent() +
                    """"platform":"linux","online":false}],"stale":false}}""",
            )
        }
        val client = IpcClient(fake, CoroutineScope(SupervisorJob()), backoff = { 25L })
        try {
            val v = withTimeout(10_000) { client.devices().getOrThrow() }
            assertEquals(1, v.devices.size)
            assertEquals("d1", v.devices[0].deviceId)
            assertNull("旧 daemon 缺 created_at → null", v.devices[0].createdAt)
        } finally {
            client.close()
        }
    }

    @Test
    fun `request code echo_off 变体——产线无回显空对象容忍`() = runBlocking {
        // final review C1:产线邮件直投模式 request_code 应答为 `{}`(码走
        // 邮件),code 必须容缺省为空串(空 = 不自动预填),不得契约违规。
        // 主形态已由 forms 锁,此处只锁变体帧(变体 req 不入库)。
        val fake = FakeDaemon()
        fake.onLine = { conn, line ->
            val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
            conn.sendLine(Goldens.withId(Goldens.read("login.request_code.resp.echo_off.json"), id))
        }
        val client = IpcClient(fake, CoroutineScope(SupervisorJob()), backoff = { 25L })
        try {
            val v = withTimeout(10_000) {
                client.loginRequestCode("http://127.0.0.1:39731", "contract@example.com").getOrThrow()
            }
            assertEquals("", v.code)
        } finally {
            client.close()
        }
    }

    @Test
    fun `扫码授权变体——claim 死码 typed 错误与 approve 过期投影`() = runBlocking {
        // Ruling R8:claim 死码(404)→ typed not_found 错误帧;approve 单次
        // 裁决重放/过期(409)→ expired 状态投影(非 RPC 错)。主形态已由
        // forms 锁,此处只锁变体帧(变体 req 不入库)。
        val fakeClaim = FakeDaemon()
        fakeClaim.onLine = { conn, line ->
            val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
            conn.sendLine(Goldens.withId(Goldens.read("qr.claim.resp.expired.json"), id))
        }
        val claimClient = IpcClient(fakeClaim, CoroutineScope(SupervisorJob()), backoff = { 25L })
        try {
            val e = withTimeout(10_000) { claimClient.qrClaim("qrt-contract-dead").exceptionOrNull() } as RpcException
            assertEquals(1L, e.code)
            assertEquals("not_found", e.kind)
            assertEquals("二维码已失效,请重新扫码", e.message)
        } finally {
            claimClient.close()
        }

        val fakeApprove = FakeDaemon()
        fakeApprove.onLine = { conn, line ->
            val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
            conn.sendLine(Goldens.withId(Goldens.read("qr.approve.resp.expired.json"), id))
        }
        val approveClient = IpcClient(fakeApprove, CoroutineScope(SupervisorJob()), backoff = { 25L })
        try {
            val v = withTimeout(10_000) { approveClient.qrApprove("qrt-contract-dead", true).getOrThrow() }
            assertEquals("expired", v.status)
        } finally {
            approveClient.close()
        }
    }

    @Test
    fun `事件 golden 全量——事件流发射且类型化解析核对`() = runBlocking {
        for ((name, check) in eventChecks) {
            val fake = FakeDaemon()
            val client = IpcClient(fake, CoroutineScope(SupervisorJob()), backoff = { 25L })
            val frames = CopyOnWriteArrayList<EventFrame>()
            // 订阅落位闸:collect 入口同步注册槽位(complete 与订阅间无挂起点),
            // await 之后主协程的续体必然晚于订阅被调度——杜绝「事件先到、
            // 订阅后到」的丢帧竞态(events 无 replay,无订阅者即丢,镜像 Rust)。
            val subscribed = CompletableDeferred<Unit>()
            val collector = launch {
                subscribed.complete(Unit)
                client.events.collect { frames.add(it) }
            }
            try {
                subscribed.await()
                withTimeout(10_000) { client.connect() }
                fake.connections.single().sendLine(Goldens.read(name)) // 原样字节 = 线上事件帧
                withTimeout(10_000) { while (frames.isEmpty()) delay(25) }
                check(frames[0])
            } catch (e: Throwable) {
                throw AssertionError("事件 $name 失败: ${e.message}", e)
            } finally {
                collector.cancel()
                client.close()
            }
        }
    }

    @Test
    fun `全量 golden 文件解析校验且无孤儿遗漏`() {
        val all = Goldens.allNames()
        // 每个文件必须可解析为 JSON(畸形 golden → 红)
        all.forEach { name ->
            val v = JVal.parse(Goldens.read(name).toString(Charsets.UTF_8))
            assertTrue("$name 须为 JSON 对象", v is JVal.Obj)
        }
        // 消费面:客户端形态 req/resp + android 变体 + 形态样本 + 登录步进
        // 变体 + 扫码授权变体 + request_code echo_off 变体 + 事件 + 不移植
        // 形态 req/resp
        val consumed = buildSet {
            forms.keys.forEach { add("$it.req.json"); add("$it.resp.json") }
            addAll(androidVariants)
            addAll(shapeVariants)
            addAll(loginStepVariants)
            addAll(qrApprovalVariants)
            addAll(requestCodeVariants)
            addAll(eventChecks.keys)
            notPortedForms.forEach { add("$it.req.json"); add("$it.resp.json") }
        }
        val leftover = all.filter { it !in consumed }
        assertTrue(
            "golden 目录出现测试未消费的新文件(孤儿 golden 盲区防护): $leftover",
            leftover.isEmpty(),
        )
        // 反向:声明的消费面文件必须真实存在(防声明与目录脱节)
        consumed.forEach { name ->
            assertTrue("声明的消费面文件缺失: $name", name in all)
        }
    }

    @Test
    fun `规范写出与 serde json 字节口径一致(键序与转义)`() {
        // 键字母序(含 CJK 键按 UTF-8 字节序)+ 转义:仅 " \ 与 <0x20 控制
        // 字符(\b\f\n\r\t 及 \u00xx 小写十六进制);非 ASCII 原样直出。
        // 值 "a" = 引号、反斜杠、换行、0x01 四个字符的转义面。
        // 转义压力输入:引号、反斜杠、换行、0x01(按码点拼装,避免字面量转义歧义)
        val tricky = buildString {
            append('"')
            append('\\')
            append(0x0A.toChar())
            append(0x01.toChar())
        }
        val value = JVal.Obj(
            "b" to JVal.Arr(listOf(JVal.num(1), JVal.Num("2.5"), JVal.Null, JVal.bool(true))),
            "a" to JVal.Str(tricky),
            "中" to JVal.Str("文"),
        )
        val expected =
            "{\"a\":\"\\\"\\\\\\n\\u0001\",\"b\":[1,2.5,null,true],\"中\":\"文\"}"
        assertEquals(expected, value.toCanonicalJson())
    }
}
