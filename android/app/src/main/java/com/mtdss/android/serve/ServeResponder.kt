package com.mtdss.android.serve

import android.util.Log
import com.mtdss.android.fdpass.FdPassChannel
import com.mtdss.android.fdpass.FdWire
import com.mtdss.android.inbox.SafInboxStore
import com.mtdss.android.ipc.DirEntry
import com.mtdss.android.ipc.EventFrame
import com.mtdss.android.ipc.IpcClient
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.ListResult
import com.mtdss.android.ipc.Ops
import com.mtdss.android.ipc.RootSpec
import com.mtdss.android.ipc.asLongOrNull
import com.mtdss.android.ipc.asStringOrNull
import com.mtdss.android.ipc.opt
import com.mtdss.android.send.RootLookup
import com.mtdss.android.send.SafFace
import com.mtdss.android.send.SourceRefs
import com.mtdss.android.send.StoredRoot
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.IOException

/**
 * roots.set 注册集(V3-4 §4.1 实施面 3):收件箱根恒在首项(`/收件箱`,
 * bridge + 可见)+ 授权根逐个映射(`vpath = "/" + 显示名`,顺序保授权表
 * 序)。空授权根 = 单收件箱(App 恒注册收件箱面,不落空集直通)。
 * 消费面 = [RootsSync](daemon api.rs `roots.set`)。
 */
fun rootsSpecs(
    inboxVpath: String = ServeResponder.INBOX_VPATH,
    authorizedRoots: List<StoredRoot>,
): List<RootSpec> = buildList {
    add(RootSpec(name = "收件箱", path = inboxVpath, source = "bridge", visible = true))
    authorizedRoots.forEach { root ->
        add(RootSpec(name = root.displayName, path = "/${root.displayName}", source = "bridge", visible = true))
    }
}

/** 收件箱授权树 URI 查询缝([com.mtdss.android.inbox.InboxGrantStore].treeUri 同形;JVM 注入)。 */
fun interface InboxGrantLookup {
    fun treeUri(): String?
}

/** 文档删除缝(App 属主 SAF deleteDocument = **硬删**,MIUI 语义库口径;
 * 实机 = DocumentsFace.deleteDocument 同形;JVM 注入)。 */
fun interface DocumentDeletion {
    fun deleteDocument(treeUri: String, documentId: String): Boolean
}

/** 导出拷贝缝:文档 URI 原始字节 → dest,实拷字节数与 expectedSize 对账
 * (不符抛 [IOException] = 尺寸守卫;调用方删临时收口)。 */
fun interface ExportCopy {
    fun copy(uri: String, dest: File, expectedSize: Long)
}

/** 服务列表条目(0x08 entries 逐项的 App 侧投影;children 仅目录项 = 直接子项数)。 */
data class ServeEntryData(
    val name: String,
    val isDir: Boolean,
    val size: Long,
    val mtimeMs: Long,
    val children: Long? = null,
) {
    /** 引擎 DirEntry 投影(本机收件箱 lister 消费;path = 完整 vpath)。 */
    internal fun toDirEntry(parentVpath: String): DirEntry = DirEntry(
        name = name,
        path = "$parentVpath/$name",
        size = size,
        mtimeMs = mtimeMs,
        isDir = isDir,
        children = children,
    )
}

/** vpath 列表结果;[Refused] = 拒答词面(0x08 error / 本机 lister 失败词)。 */
sealed interface ServeListing {
    data class Ok(val entries: List<ServeEntryData>, val truncated: Boolean = false) : ServeListing
    data class Refused(val reason: String) : ServeListing
}

/**
 * 服务桥应答器(V3-4 §4.1 安卓服务桥的安卓正身):消费 daemon 的
 * `fs.serve_list` / `fs.serve_export` / `fs.serve_delete` 事件,经 fdpass
 * **服务通道**(0x08/0x09/0x0A,专用实例)应答——引擎把 roots.set 注册的
 * bridge vpath(收件箱 `/收件箱` + SAF 授权根)逐请求实问本类,SAF 真面在
 * App 进程(引擎读不了 content://)。
 *
 * - **列表**(0x08):vpath(root + sub)→ SAF 树下钻
 *   ([SourceRefs.drillChildDocumentId] 逐段,全 provider 成立)→ 直接子项
 *   (目录携 children = 直接子项数)。收件箱根走收件根归一化(授权树根名 =
 *   MTDSS → 根即收件箱;否则树内 MTDSS 子目录,纯查询零副作用,与
 *   SafInboxStore 发布同语义);授权根按显示名 vpath 命中 [RootLookup]
 *   实时表(增删即生效,无缓存)。`.mtdss` 私有记账区在收件箱面剔除
 *   (§4.4 暂存区对端不可见语义)。单帧帽 [LISTING_CAP](64 KiB payload
 *   上限防协议断连;超出 truncated=true,与 fs 列表截断语义一致)。
 * - **导出**(0x09):原始字节(SAF 只读句柄流拷贝)落自有临时目录
 *   `{home}/serve-tmp/`(`<requestId>-<名>` 请求级唯一名)回绝对路径——
 *   引擎按普通文件读取/发送并在传输终态后 unlink;**尺寸守卫**:预期值取
 *   父列表条目元数据,实拷不符(盘损/中途失效)删临时拒导。
 * - **删除**(0x0A):SAF deleteDocument 一次调用 = 整树语义(App 属主 =
 *   硬删);目标缺席 = deleted:false(引擎收 NOT_FOUND)。删除授权随
 *   持久授权旗——只读授权的树删除被 provider 拒 = ok:false 诚实失败。
 * - **启动清扫**:挂载即清空 serve-tmp(崩溃残件自愈,与引擎侧「终态
 *   unlink」两侧兜底;iOS photo-serve 同哲学)。
 *
 * 生命周期:AppGraph 构造即订阅(先于任何屏面动作,SourceProvider 同纪律);
 * 畸形事件帧静默丢弃不炸订阅;应答帧 ack ok:false = 请求已超时/未知,记
 * 日志即毕。
 *
 * **本机收件箱 lister**([listInbox],T10 接缝):快捷目录「收件箱」节点的
 * 数据面 = 同一 list 面的 [DirEntry] 投影(引擎 recv_dir 实机为收件哨兵,
 * 非真路径;SAF 真面经此直喂 BrowseViewModel,不经引擎)。
 */
class ServeResponder(
    private val saf: SafFace,
    private val inboxGrant: InboxGrantLookup,
    private val roots: RootLookup,
    /** 导出临时目录锚 = {engine_home}/serve-tmp(MtdssEngine.serveTmpDir;
     * 与 daemon serve::serve_dir 同锚,两侧不得漂移)。 */
    private val serveDir: File,
    private val deletion: DocumentDeletion,
    private val exportCopy: ExportCopy,
    private val channel: FdPassChannel,
    ipc: IpcClient,
    private val scope: CoroutineScope,
    private val ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
) {
    private val subscribed = CompletableDeferred<Unit>()

    /** 单帧条目帽(64 KiB payload 上限内;超出 truncated=true)。 */
    companion object {
        /** 收件箱 vpath(daemon 侧注册名/路由根;rootsSpecs 与本类同锚)。 */
        const val INBOX_VPATH: String = "/收件箱"

        /** 列表单帧条目帽(条目 ~120B → 256 项 ≈ 30KiB,64KiB 内安全)。 */
        const val LISTING_CAP: Int = 256

        /** 本桥消费的事件集(订阅过滤;其余事件各屏面自消费)。 */
        private val SERVE_EVENTS = setOf(Ops.EVENT_SERVE_LIST, Ops.EVENT_SERVE_EXPORT, Ops.EVENT_SERVE_DELETE)

        /** 收件根目录名(授权树内归一化目标;与 SafInboxStore 同一常量源)。 */
        private val INBOX_DIR_NAME = SafInboxStore.INBOX_DIR_NAME

        /** App 私有记账区名(收件箱服务面剔除;.mtdss/MIUI 改写双形态)。 */
        private val PRIVATE_DIR_NAMES = SafInboxStore.STAGING_DIR_NAMES

        private const val TAG = "ServeResponder"
    }

    init {
        scope.launch {
            // 启动清扫(App 侧 unlink 义务;崩溃残件自愈)
            runCatching { sweep() }.onFailure { Log.w(TAG, "serve-tmp 清扫失败: ${it.message}") }
            ipc.events
                .onSubscription { subscribed.complete(Unit) }
                .collect { frame ->
                    if (frame.method in SERVE_EVENTS) {
                        scope.launch(ioDispatcher) {
                            runCatching { handle(frame) }
                                .onFailure { Log.w(TAG, "serve 事件处置异常 ${frame.method}: ${it.message}") }
                        }
                    }
                }
        }
    }

    /** 订阅落位闸(测试/复用方显式同步;运行时构造序已保证)。 */
    suspend fun awaitSubscribed() = subscribed.await()

    /** 单事件处置(测试直驱面;阻塞 SAF 面在调用方线程)。 */
    suspend fun handle(frame: EventFrame) {
        val requestId = try {
            frame.params.opt("request_id")?.asLongOrNull()
        } catch (e: Exception) {
            null
        }
        if (requestId == null) {
            Log.w(TAG, "${frame.method} 帧缺 request_id,丢弃")
            return
        }
        when (frame.method) {
            Ops.EVENT_SERVE_LIST -> {
                val root = frame.params.opt("root")?.asStringOrNull() ?: return refuse(channel, FdWire.KIND_SERVE_LIST, requestId, "事件缺 root")
                val sub = frame.params.opt("sub")?.asStringOrNull() ?: return refuse(channel, FdWire.KIND_SERVE_LIST, requestId, "事件缺 sub")
                when (val listing = listVpath(root, sub)) {
                    is ServeListing.Ok -> reply(
                        channel, FdWire.KIND_SERVE_LIST, requestId,
                        FdWire.listReplyPayload(requestId, listing.entries.map(::serveEntryPayload), listing.truncated),
                    )

                    is ServeListing.Refused -> refuse(channel, FdWire.KIND_SERVE_LIST, requestId, listing.reason)
                }
            }

            Ops.EVENT_SERVE_EXPORT -> {
                val root = frame.params.opt("root")?.asStringOrNull() ?: return refuse(channel, FdWire.KIND_SERVE_EXPORT, requestId, "事件缺 root")
                val sub = frame.params.opt("sub")?.asStringOrNull() ?: return refuse(channel, FdWire.KIND_SERVE_EXPORT, requestId, "事件缺 sub")
                exportTo(root, sub, requestId).fold(
                    onSuccess = { f -> reply(channel, FdWire.KIND_SERVE_EXPORT, requestId, FdWire.exportReplyPayload(requestId, f.absolutePath)) },
                    onFailure = { e -> refuse(channel, FdWire.KIND_SERVE_EXPORT, requestId, e.message ?: "导出失败") },
                )
            }

            Ops.EVENT_SERVE_DELETE -> {
                val root = frame.params.opt("root")?.asStringOrNull() ?: return refuse(channel, FdWire.KIND_SERVE_DELETE, requestId, "事件缺 root")
                val sub = frame.params.opt("sub")?.asStringOrNull() ?: return refuse(channel, FdWire.KIND_SERVE_DELETE, requestId, "事件缺 sub")
                val deleted = deletePath(root, sub)
                reply(channel, FdWire.KIND_SERVE_DELETE, requestId, FdWire.deleteReplyPayload(requestId, deleted))
            }

            else -> Unit // 非本桥事件(各屏订阅面互不抢流)
        }
    }

    // ── 列表(0x08 + 本机收件箱 lister 共用面)──────────────────────────────

    /**
     * vpath 列表:root(收件箱/授权根 vpath)+ sub(根下子路径,"" = 根自身)
     * → SAF 直接子项。SAF 阻塞面,调用方在 IO 线程。
     */
    fun listVpath(root: String, sub: String): ServeListing {
        val tree = when (val resolved = resolveRoot(root)) {
            is RootResolved.Ok -> resolved
            is RootResolved.Refused -> return ServeListing.Refused(resolved.reason)
        }
        if (!subOk(sub)) return ServeListing.Refused("非法子路径: $sub")
        val docId = if (sub.isEmpty()) {
            tree.baseDocId
        } else {
            SourceRefs.drillChildDocumentId(tree.baseDocId, sub) { parent ->
                saf.treeChildren(tree.treeUri, parent)
            } ?: return ServeListing.Refused("无此目录: $root/$sub")
        }
        val kids = saf.treeChildren(tree.treeUri, docId)
            ?: return ServeListing.Refused("目录读取失败: $root/$sub")
        val visible = if (tree.isInbox) kids.filter { it.name !in PRIVATE_DIR_NAMES } else kids
        val truncated = visible.size > LISTING_CAP
        val entries = visible.take(LISTING_CAP).map { kid ->
            ServeEntryData(
                name = kid.name,
                isDir = kid.isDir,
                size = kid.size,
                mtimeMs = kid.mtimeMs,
                // treeChildren 恒列直接子项(计数即 size;收件箱面按剔除后计)
                children = if (kid.isDir) {
                    val direct = if (tree.isInbox) {
                        saf.treeChildren(tree.treeUri, kid.documentId)
                            ?.count { it.name !in PRIVATE_DIR_NAMES }
                    } else {
                        saf.treeChildren(tree.treeUri, kid.documentId)?.size
                    }
                    direct?.toLong()
                } else {
                    null
                },
            )
        }
        return ServeListing.Ok(entries, truncated)
    }

    /**
     * 本机收件箱 lister(T10 接缝):收件箱 vpath 子树 → 引擎
     * [ListResult](条目 path = 完整 vpath,可直接再下钻/推整页)。非收件箱
     * vpath(含 `/收件箱x` 前缀撞形)拒;未授权/树不可达 = 失败词面直投
     * BrowseViewModel 错误态。
     */
    suspend fun listInbox(vpath: String): Result<ListResult> = withContext(ioDispatcher) {
        val sub = when {
            vpath == INBOX_VPATH -> ""
            vpath.startsWith("$INBOX_VPATH/") -> vpath.removePrefix("$INBOX_VPATH/")
            else -> return@withContext Result.failure(IllegalArgumentException("非收件箱路径: $vpath"))
        }
        when (val listing = listVpath(INBOX_VPATH, sub)) {
            is ServeListing.Ok -> {
                val parent = INBOX_VPATH + if (sub.isEmpty()) "" else "/$sub"
                Result.success(ListResult(entries = listing.entries.map { it.toDirEntry(parent) }, truncated = listing.truncated))
            }

            is ServeListing.Refused -> Result.failure(IllegalStateException(listing.reason))
        }
    }

    // ── 导出(0x09)────────────────────────────────────────────────────────

    /**
     * 导出一件:原始字节 → `serveDir/<requestId>-<名>`(请求级唯一名)。
     * 尺寸守卫双道:拷贝面等长对账 + 写后 len 复核;失败删临时收口(引擎侧
     * 另有传输终态 unlink,两侧兜底)。
     */
    fun exportTo(root: String, sub: String, requestId: Long): Result<File> {
        val tree = when (val resolved = resolveRoot(root)) {
            is RootResolved.Ok -> resolved
            is RootResolved.Refused -> return Result.failure(IllegalStateException(resolved.reason))
        }
        if (sub.isEmpty() || !subOk(sub)) return Result.failure(IllegalStateException("非法导出路径: $root/$sub"))
        // 父列表定位条目(名 + 尺寸守卫预期值;目录/缺席拒)
        val name = sub.substringAfterLast('/')
        val parentSub = parentSub(sub)
        val parentDoc = if (parentSub.isEmpty()) {
            tree.baseDocId
        } else {
            SourceRefs.drillChildDocumentId(tree.baseDocId, parentSub) { parent ->
                saf.treeChildren(tree.treeUri, parent)
            } ?: return Result.failure(IllegalStateException("无此目录: $root/$parentSub"))
        }
        val hit = saf.treeChildren(tree.treeUri, parentDoc)?.firstOrNull { it.name == name }
            ?: return Result.failure(IllegalStateException("源已不存在: $root/$sub"))
        if (hit.isDir) return Result.failure(IllegalStateException("目录不可导出: $root/$sub"))
        serveDir.mkdirs()
        val dest = File(serveDir, "$requestId-$name")
        return try {
            exportCopy.copy(saf.buildChildUri(tree.treeUri, hit.documentId), dest, hit.size)
            if (dest.length() != hit.size) {
                dest.delete()
                Result.failure(IOException("导出尺寸不符(实落 ${dest.length()} 预期 ${hit.size}): $root/$sub"))
            } else {
                Result.success(dest)
            }
        } catch (e: Exception) {
            dest.delete()
            Result.failure(e)
        }
    }

    // ── 删除(0x0A)────────────────────────────────────────────────────────

    /**
     * 删除一件(整树语义:SAF deleteDocument 一次调用,App 属主 = 硬删);
     * 目标缺席/根未注册/子路径非法 = false(引擎收 NOT_FOUND)。删除授权随
     * 持久授权旗(只读授权的树被 provider 拒 = false 诚实失败)。
     */
    fun deletePath(root: String, sub: String): Boolean {
        val tree = when (val resolved = resolveRoot(root)) {
            is RootResolved.Ok -> resolved
            is RootResolved.Refused -> return false
        }
        if (sub.isEmpty() || !subOk(sub)) return false
        val docId = SourceRefs.drillChildDocumentId(tree.baseDocId, sub) { parent ->
            saf.treeChildren(tree.treeUri, parent)
        } ?: return false
        return deletion.deleteDocument(tree.treeUri, docId)
    }

    // ── 内部:根解析 / 子路径防御 / 帧化 ────────────────────────────────────

    /** 根面解析结果(treeUri + 基 docId + 收件箱旗)。 */
    private sealed interface RootResolved {
        data class Ok(val treeUri: String, val baseDocId: String, val isInbox: Boolean) : RootResolved
        data class Refused(val reason: String) : RootResolved
    }

    /**
     * vpath 根名 → SAF 树面。收件箱 = 授权树收件根归一化(根名 MTDSS → 根
     * 即收件箱;否则树内 MTDSS 子目录,纯查询零副作用——与 SafInboxStore
     * 发布/在场同语义,勿漂移);授权根 = [RootLookup] 实时表按显示名 vpath
     * 命中(增删即生效,无缓存)。
     */
    private fun resolveRoot(root: String): RootResolved {
        if (root == INBOX_VPATH) {
            val treeUri = inboxGrant.treeUri()
                ?: return RootResolved.Refused("收件箱未授权:请在 App 内完成目录授权后重试")
            val treeDocId = saf.treeDocumentId(treeUri)
                ?: return RootResolved.Refused("收件箱授权无效(树 URI 解析失败)")
            val inboxDocId = inboxRootDocumentId(treeUri, treeDocId)
                ?: return RootResolved.Refused("收件根未落位(尚未收过件)")
            return RootResolved.Ok(treeUri, inboxDocId, isInbox = true)
        }
        if (!root.startsWith("/")) return RootResolved.Refused("无此根")
        val rootName = root.removePrefix("/")
        val hit = roots.roots().firstOrNull { it.displayName == rootName }
            ?: return RootResolved.Refused("无此根")
        return RootResolved.Ok(hit.treeUri, hit.treeDocId, isInbox = false)
    }

    /** 收件根只读定位(查询零副作用;归一化规则见 [resolveRoot])。 */
    private fun inboxRootDocumentId(treeUri: String, treeDocId: String): String? {
        val rootName = saf.documentMeta(saf.buildChildUri(treeUri, treeDocId))?.name ?: return null
        if (rootName.equals(INBOX_DIR_NAME, ignoreCase = true)) return treeDocId
        return saf.treeChildren(treeUri, treeDocId)
            ?.firstOrNull { it.isDir && it.name.equals(INBOX_DIR_NAME, ignoreCase = true) }?.documentId
    }

    /**
     * 子路径防御(daemon bridge_route 词法规范化后的第二道;fail-closed):
     * 空串 = 根自身合法;段非空、无 `.`/`..`/反斜杠/冒号段。
     */
    private fun subOk(sub: String): Boolean =
        sub.isEmpty() || sub.split('/').none { it.isEmpty() || it == "." || it == ".." || it.contains('\\') || it.contains(':') }

    /** 子路径 → 父子路径("" = 根;`a/b/c` → `a/b`;daemon serve::parent_sub 同形)。 */
    private fun parentSub(sub: String): String =
        sub.substringBeforeLast('/', missingDelimiterValue = "")

    /** 条目 → 0x08 线上形(`{children?, is_dir, mtime_ms, name, size}`;字母序由 toCanonicalJson 统一)。 */
    internal fun serveEntryPayload(e: ServeEntryData): JVal.Obj {
        val m = linkedMapOf<String, JVal>(
            "is_dir" to JVal.bool(e.isDir),
            "mtime_ms" to JVal.num(e.mtimeMs),
            "name" to JVal.str(e.name),
            "size" to JVal.num(e.size),
        )
        if (e.children != null) m["children"] = JVal.num(e.children)
        return JVal.Obj(m)
    }

    /** 启动清扫:清空 serve-tmp 内容(目录保留;逐项失败留痕不中断)。 */
    private fun sweep() {
        serveDir.mkdirs()
        serveDir.listFiles()?.forEach { item ->
            runCatching { item.deleteRecursively() }
                .onFailure { Log.w(TAG, "导出残件清理失败 ${item.name}: ${it.message}") }
        }
    }

    private suspend fun reply(
        channel: FdPassChannel,
        kind: Int,
        requestId: Long,
        payload: JVal.Obj,
    ) {
        channel.sendReply(kind, requestId, payload).fold(
            onSuccess = { ack -> if (!ack.ok) Log.w(TAG, "0x%02x 应答被拒(请求已超时/未知)id=$requestId".format(kind)) },
            onFailure = { e -> Log.w(TAG, "0x%02x 应答发送失败 id=$requestId: ${e.message}".format(kind)) },
        )
    }

    private suspend fun refuse(channel: FdPassChannel, kind: Int, requestId: Long, reason: String) {
        Log.w(TAG, "服务桥拒答 kind=0x%02x id=$requestId: $reason".format(kind))
        reply(channel, kind, requestId, FdWire.refusePayload(requestId, reason))
    }
}

/**
 * 实机导出拷贝(SafFace 只读句柄 → serve-tmp 流拷贝 + 等长对账;失败抛
 * [IOException],调用方删临时收口)。
 */
class SafExportCopy(private val saf: SafFace) : ExportCopy {
    override fun copy(uri: String, dest: File, expectedSize: Long) {
        val opened = saf.openRead(uri)
        try {
            FileInputStream(opened.fd).use { input ->
                FileOutputStream(dest).use { out ->
                    val buf = ByteArray(BUFFER_BYTES)
                    var copied = 0L
                    while (true) {
                        val n = input.read(buf)
                        if (n < 0) break
                        out.write(buf, 0, n)
                        copied += n
                    }
                    if (copied != expectedSize) {
                        throw IOException("拷贝字节数不符:实拷 $copied 预期 $expectedSize")
                    }
                }
            }
        } finally {
            opened.release()
        }
    }

    private companion object {
        const val BUFFER_BYTES: Int = 8 * 1024 * 1024
    }
}
