package com.mtdss.android.share

import android.util.Log
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asArray
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.asString
import com.mtdss.android.ipc.opt
import com.mtdss.android.ipc.req
import com.mtdss.android.ipc.toCanonicalJson
import com.mtdss.android.send.OpenedSource
import java.io.File
import java.io.FileInputStream
import java.io.IOException
import java.io.InputStream
import kotlin.random.Random

/**
 * 分享暂存队列(V3-4 spec §4.4 T15;iOS ShareQueueStore/方案 A 语义镜像):
 *
 * **形态**:App 私有暂存区 `{filesDir}/share/queue/<item>/` 为队列项(item
 * 目录,**引擎 home 外**——不注入引擎路径命名空间、不进授权集、对端不可见),
 * `files/` 下两步拷贝(`.part` 临时名 → 同目录 rename 完成),`meta.json`
 * 记意图(目标设备 + 声明尺寸 + 状态,蛇形键 JVal 规范化落盘)。
 *
 * **两步拷贝与失败语义(评审裁定:数据丢失形状的意外不可接受)**:
 * - meta 先行(status=.copying,声明逐文件 size)→ 逐文件 `.part` →
 *   rename → 全部落定翻 .ready。分享 Activity 中途被杀:.copying 项在宽限窗
 *   ([DEFAULT_COPY_GRACE_MS],防主 App 消费撞上仍在跑的拷贝)外判
 *   「拷贝中断」失败态;孤儿目录(无 meta)同判。失败行跨会话不丢(折算
 *   落盘)。
 * - 拷中尺寸守卫:provider 流字节数 ≠ 声明即拒收(坏件不入队,整项移除)。
 *
 * **发送形态(§4.4 裁定,与 iOS 分叉点)**:暂存件走**桥式**(0x01
 * request_source,App 作源;ref = `stage://<itemId>/<name>`,SourceProvider
 * 解析后直开暂存文件 fd)。故文件**不迁收件箱**,队列目录即暂存区;受理
 * 成功翻 .sent(文件保暂存供引擎逐文件拉取),超窗([DEFAULT_SENT_SWEEP_MS])
 * 由 scan 清扫(残留有界;完成与否归引擎任务列表)。
 *
 * 宿主可测:纯 JVM(root 临时目录注入;send 闭包注入见 [ShareQueueConsumer])。
 */
class ShareQueueStore(private val root: File) {

    /** 队列根 = `{root}/queue/`。 */
    val queueDir: File get() = File(root, "queue")

    /** 单项目录 = `{queue}/<id>/`;文件目录 = `{item}/files/`。 */
    fun itemDir(id: String): File = File(queueDir, id)

    fun filesDir(id: String): File = File(itemDir(id), "files")

    fun metaFile(id: String): File = File(itemDir(id), "meta.json")

    // ── 写入面(分享 Activity)────────────────────────────────────────────

    /**
     * 开项:建目录 + 写 meta(status=.copying,声明逐文件 size)。meta 原子
     * 写(临时名+rename),在场即可被消费端识别为「拷贝中」。name 统一净化
     * (显示名可能带路径分隔/冒号)。
     */
    fun beginItem(id: String, targetDeviceId: String, files: List<ShareQueueFile>, nowMs: Long) {
        val sanitized = files.map { ShareQueueFile(sanitizedName(it.name), it.size) }
        filesDir(id).mkdirs()
        val meta = ShareQueueMeta(
            targetDeviceId = targetDeviceId,
            sourceName = sanitized.firstOrNull()?.name ?: "",
            size = sanitized.sumOf { it.size },
            enqueuedAt = nowMs,
            status = ShareQueueStatus.COPYING,
            files = sanitized,
        )
        writeMeta(meta, id)
    }

    /**
     * 单文件两步拷贝(流源形,分享件实路):`.part` 临时名 → 计数落盘 →
     * **拷后校验**(实收字节 ≠ 声明即拒收删 .part,坏件不入队)→ rename
     * 完成(P4a 语义——最终名在场即完整)。
     */
    fun copyStream(id: String, file: ShareQueueFile, input: InputStream) {
        val name = sanitizedName(file.name)
        requireValidStagedName(name)
        val dest = File(filesDir(id), name)
        val tmp = File(filesDir(id), "$name.part")
        var actual = 0L
        try {
            input.use { ins ->
                tmp.outputStream().use { out ->
                    val buf = ByteArray(COPY_BUFFER)
                    var r: Int
                    while (ins.read(buf).also { r = it } >= 0) {
                        out.write(buf, 0, r)
                        actual += r
                    }
                }
            }
        } catch (e: IOException) {
            tmp.delete()
            throw e
        }
        if (actual != file.size) {
            tmp.delete()
            throw ShareQueueSizeMismatchException(name = name, declared = file.size, actual = actual)
        }
        dest.delete()
        if (!tmp.renameTo(dest)) {
            tmp.delete()
            throw IOException("暂存落定失败:$name")
        }
    }

    /** 收项:全部文件落定后翻 .ready(消费端放行)。 */
    fun finishItem(id: String, nowMs: Long) {
        val meta = readMeta(id) ?: throw ShareQueueMetaMissingException(id)
        writeMeta(meta.copy(status = ShareQueueStatus.READY), id)
    }

    // ── 读取/状态面(主 App 消费面)──────────────────────────────────────

    /** 读单项 meta(缺失/损坏 = null,消费端按孤儿目录收口)。 */
    fun readMeta(id: String): ShareQueueMeta? = try {
        ShareQueueMeta.decode(metaFile(id).readText())
    } catch (_: Exception) {
        null
    }

    /**
     * 扫描全队列(copying 宽限窗外与孤儿目录折算失败态并**持久化**,失败行
     * 跨会话不丢;sent 超窗清扫整项——暂存目录有界)。序按 enqueuedAt 升序
     * (先来先消费)。
     */
    fun scan(
        nowMs: Long,
        copyGraceMs: Long = DEFAULT_COPY_GRACE_MS,
        sentSweepMs: Long = DEFAULT_SENT_SWEEP_MS,
    ): List<ShareQueueEntry> {
        val dirs = queueDir.listFiles { f -> f.isDirectory } ?: emptyArray()
        val entries = ArrayList<ShareQueueEntry>(dirs.size)
        for (dir in dirs.sortedBy { it.name }) {
            val id = dir.name
            val meta = readMeta(id)
            if (meta == null) {
                // 孤儿目录(meta 未写/损坏)→ 折算失败并落盘(不静默丢)
                val failed = ShareQueueMeta(
                    targetDeviceId = "", sourceName = "", size = 0,
                    enqueuedAt = dir.lastModified(), status = ShareQueueStatus.FAILED,
                    files = emptyList(), error = ShareQueueConsumer.ERROR_COPY_INTERRUPTED,
                )
                runCatching { writeMeta(failed, id) }
                entries += ShareQueueEntry(id, failed)
                continue
            }
            when {
                meta.status == ShareQueueStatus.COPYING && nowMs - meta.enqueuedAt > copyGraceMs -> {
                    // 分享面已死(宽限窗外仍在 copying)→ 折算失败并落盘
                    val failed = meta.copy(
                        status = ShareQueueStatus.FAILED,
                        error = ShareQueueConsumer.ERROR_COPY_INTERRUPTED,
                    )
                    runCatching { writeMeta(failed, id) }
                    entries += ShareQueueEntry(id, failed)
                }

                meta.status == ShareQueueStatus.SENT && nowMs - meta.enqueuedAt > sentSweepMs -> {
                    // 受理件超窗:引擎拉取期早过(正常作业秒级起步)→ 整项清扫
                    runCatching {
                        itemDir(id).deleteRecursively()
                        Log.i(TAG, "分享暂存项已清扫 id=$id age=${nowMs - meta.enqueuedAt}ms")
                    }
                }

                else -> entries += ShareQueueEntry(id, meta)
            }
        }
        return entries.sortedWith(compareBy({ it.meta.enqueuedAt }, { it.id }))
    }

    /** 标失败(原因落盘)。 */
    fun markFailed(id: String, error: String) {
        val meta = readMeta(id) ?: throw ShareQueueMetaMissingException(id)
        writeMeta(meta.copy(status = ShareQueueStatus.FAILED, error = error), id)
    }

    /**
     * 标受理完成(发送已受理翻 .sent;文件保暂存供引擎 request_source 逐文件
     * 拉取——**不移除项**,清扫交 [scan] 超窗)。
     */
    fun markSent(id: String) {
        val meta = readMeta(id) ?: throw ShareQueueMetaMissingException(id)
        writeMeta(meta.copy(status = ShareQueueStatus.SENT), id)
    }

    /** 清队列项(失败行「清除」;暂存件一并回收)。不存在幂等。 */
    fun removeItem(id: String) {
        itemDir(id).deleteRecursively()
    }

    /**
     * meta 原子写(临时名 + rename;写读两侧同进程,原子性防的是撞上半截
     * JSON / 拷贝中被消费端读到未完态。Windows 语义:目标在场须先删——
     * java.io.File.renameTo 不做替换,POSIX 才有 replace-on-rename)。
     */
    private fun writeMeta(meta: ShareQueueMeta, id: String) {
        itemDir(id).mkdirs()
        val dest = metaFile(id)
        val tmp = File(itemDir(id), "meta.json.tmp")
        tmp.writeText(meta.encode())
        dest.delete()
        if (!tmp.renameTo(dest)) {
            tmp.delete()
            throw IOException("meta 落盘失败:$id")
        }
    }

    // ── 暂存件解析(SourceProvider 0x01 应答 / 重发起重建共面)────────────

    /** 暂存文件(在场才回;越名/越 id/缺件 = null → 拒开「源不可用」)。 */
    fun stagedFile(id: String, name: String): File? {
        // id 同名单段守卫(解析面来的 id 防目录穿越:`..`/带分隔拒)
        if (!isValidStagedName(id) || !isValidStagedName(name)) return null
        val f = File(filesDir(id), name)
        return if (f.isFile) f else null
    }

    /** 暂存文件读句柄(0x01 source_fd 交付;SCM_RIGHTS 在 sendmsg 时复制)。 */
    fun openStagedSource(id: String, name: String): OpenedSource? {
        val f = stagedFile(id, name) ?: return null
        val ins = FileInputStream(f)
        return OpenedSource(ins.fd) { runCatching { ins.close() } }
    }

    companion object {

        private const val TAG = "ShareQueue"

        /** 拷贝流缓冲(暂存拷贝用;小文件单循环)。 */
        private const val COPY_BUFFER = 64 * 1024

        /**
         * 拷贝中断判定宽限窗(毫秒):meta=.copying 且入队早于此时点才判失败
         * ——分享面板入队后即拉起消费,窗口只防「用户手动开 App 撞上仍在拷」
         * 的窄竞窗(iOS defaultCopyGrace 120s 同值)。
         */
        const val DEFAULT_COPY_GRACE_MS: Long = 120_000L

        /**
         * 受理件清扫窗(毫秒):.sent 项超窗整项移除(暂存目录有界;发送受理
         * 后引擎逐文件拉源,正常作业秒级起步,窗内文件保暂存即足)。重发起
         * 同窗:超窗后分享件已清理,中断行重发起按「源已不存在」收口(可再
         * 分享)。
         */
        const val DEFAULT_SENT_SWEEP_MS: Long = 24 * 60 * 60 * 1000L

        /**
         * 文件名净化(iOS sanitizedName 同锚:目录分量只取 '/' 末段;分隔
         * 面字符 `\`/`:` 净化为 `_`;相对分量 `.`/`..` 净化为 `_`——暂存区
         * 目录穿越防)。
         */
        fun sanitizedName(name: String): String {
            val last = name.substringAfterLast('/')
            val stripped = last.replace(":", "_").replace("\\", "_")
            return if (stripped == "." || stripped == "..") "_" else stripped
        }

        /** 暂存件名守卫(单段;`.`/`..`/带分隔拒)。 */
        fun isValidStagedName(name: String): Boolean =
            name.isNotEmpty() && name != "." && name != ".." &&
                '/' !in name && '\\' !in name

        private fun requireValidStagedName(name: String) {
            require(isValidStagedName(name)) { "暂存件名非法:$name" }
        }

        /** 新项 id:`s<epoch 毫秒>-<4hex>`(同毫秒并发分享靠随机段避撞)。 */
        fun newItemId(nowMs: Long, random: Random = Random.Default): String {
            val hex = (0 until 4).map { "0123456789abcdef"[random.nextInt(16)] }.joinToString("")
            return "s$nowMs-$hex"
        }
    }
}

/** 队列项单文件声明(入队时刻的净化名/声明尺寸;消费时逐文件对实际校验)。 */
data class ShareQueueFile(val name: String, val size: Long)

/** 队列项状态机:copying(拷贝中)→ ready(待消费)→ sent(已受理)/ failed(可重试)。 */
enum class ShareQueueStatus(val wire: String) {
    COPYING("copying"),
    READY("ready"),
    SENT("sent"),
    FAILED("failed");

    companion object {
        /** 解码容错:未知/损坏状态按 failed 收口——不静默丢(iOS 同口径)。 */
        fun fromWire(s: String?): ShareQueueStatus =
            entries.firstOrNull { it.wire == s } ?: FAILED
    }
}

/**
 * 队列项意图(蛇形键落盘:{target_device_id, source_name, size, enqueued_at,
 * status, files[{name,size}], error};与 iOS ShareQueueMeta schema 同语言,
 * epoch 毫秒整型)。error = 失败原因(「文件拷贝中断」「文件可能已损坏」或
 * 引擎错误原文)。
 */
data class ShareQueueMeta(
    val targetDeviceId: String,
    val sourceName: String,
    val size: Long,
    val enqueuedAt: Long,
    val status: ShareQueueStatus,
    val files: List<ShareQueueFile>,
    val error: String? = null,
) {
    fun encode(): String = JVal.Obj(
        "target_device_id" to JVal.str(targetDeviceId),
        "source_name" to JVal.str(sourceName),
        "size" to JVal.num(size),
        "enqueued_at" to JVal.num(enqueuedAt),
        "status" to JVal.str(status.wire),
        "files" to JVal.Arr(files.map { JVal.Obj("name" to JVal.str(it.name), "size" to JVal.num(it.size)) }),
        "error" to (error?.let { JVal.str(it) } ?: JVal.Null),
    ).toCanonicalJson()

    companion object {
        /** 解码(必填字段缺/类型错抛;调用方 catch 整单按孤儿收口)。 */
        fun decode(json: String): ShareQueueMeta {
            val o = JVal.parse(json)
            return ShareQueueMeta(
                targetDeviceId = o.req("target_device_id").asString(),
                sourceName = o.req("source_name").asString(),
                size = o.req("size").asLong(),
                enqueuedAt = o.req("enqueued_at").asLong(),
                status = ShareQueueStatus.fromWire(o.opt("status")?.let {
                    runCatching { it.asString() }.getOrNull()
                }),
                files = o.req("files").asArray().map { f ->
                    ShareQueueFile(name = f.req("name").asString(), size = f.req("size").asLong())
                },
                error = o.opt("error")?.let { runCatching { it.asString() }.getOrNull() },
            )
        }
    }
}

/** scan 出口(id + meta)。 */
data class ShareQueueEntry(val id: String, val meta: ShareQueueMeta)

/** 拷中尺寸守卫拒收(provider 流实收 ≠ 声明;坏件不入队)。 */
class ShareQueueSizeMismatchException(val name: String, val declared: Long, val actual: Long) :
    Exception("尺寸不符:$name 声明 $declared 实收 $actual")

/** meta 缺失(状态迁移面防御;队列项损坏形态)。 */
class ShareQueueMetaMissingException(val id: String) : Exception("队列项 meta 缺失: $id")
