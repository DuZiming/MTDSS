package com.mtdss.android.inbox

import android.util.Log
import android.webkit.MimeTypeMap
import com.mtdss.android.send.SourceRefs
import java.io.FileInputStream
import java.io.IOException
import java.util.UUID

/**
 * 收件箱发布的 SAF 授权树面(P4a-r2:收件操作从 MediaStore 整体迁至 SAF,
 * spec §7;取代已退役的 MediaStoreInboxStore/MediaFace——App 不再持有任何
 * MediaStore 行,发布后可见性交系统扫描器自然索引,spec §7.6)。实现保持
 * 薄:发布编排与在场查询的决策在本类 JVM 可验,Android 存储行为收进
 * [DocumentsFace]/[ContentResolverDocumentsFace] 缝后实机验证:
 *
 * - **收件根归一化**(spec §7.2):授权树根名 = MTDSS → 根即收件箱;否则
 *   在树内定位/自建 `MTDSS` 子目录(createDirectory,存在则用)——用户授
 *   `Download/MTDSS`(最小权限)或 `Download` 皆可,rel 语义不变(目录
 *   结构保真,嵌套段逐级自建);
 * - **发布编排**(spec §7.3/§7.5,决策矩阵 JVM 钉死):①落位子项预查
 *   (同名**目录**占用 → 拒;真文件在场沿 [TransferPolicy]:skip 命中弃件
 *   记 Skipped,其余 SAF delete 驱逐)→ ②`MTDSS/.mtdss/` 隐藏目录内
 *   createDocument **点文件临时名** `.mtdss-pub-<8hex>.tmp` → ③流拷贝
 *   (8 MiB 缓冲 + 实拷等长对账)→ ④moveDocument 临时件移入落位父目录
 *   (rename 不换父目录,落位的移动半步;仍以随机临时名行进,不撞终名)
 *   → ⑤renameDocument 落正式名(**返回名 ≠ 请求名 = 碰撞**,沿 F14b
 *   词面 Failed 诚实告知,改名后的正式文件保留 = 行保留等价物)→
 *   ⑥move/rename 返回新 id 时显式删旧 id(provider 复制式变更的残留
 *   清理;移动式旧 id 已失效,删除无害失败);②③④⑤任一失败(活进程
 *   内)SAF **删件**收 Failed。强杀残留**恒在 `.mtdss` 内**(隐藏目录,
 *   §1 原则兑现;④后强杀 = 落位父目录内的点文件临时名,不可见且下次
 *   发布可清),SAF 对授权树有删除权,GC 能力归 App(spec §7.7:MIUI
 *   若把点名改写为 `_.mtdss`,定位双形态兼容,残留仍收纳于专用目录);
 * - **在场判据(0x04,spec §7.4)**:exists 按 rel 在授权树内下钻查子
 *   文档——**文档真实存在且尺寸吻合**才是在场,比 MediaStore 行查询更真
 *   (无扫描器时延);任何失败(未授权/下钻断链/查询异常/尺寸不符)→
 *   false(保守沿 F10 铁律:宁可重传,不可假完成)。F10-F12 的僵尸行
 *   家族(行与文件失联的一切形态)在接收面整体退场——无行,只有文件;
 * - **索引**(spec §7.6):rename 成功后主动 scanFile(docId 推导路径,
 *   ExternalStorageProvider `primary:…` 形;推导不出交扫描器自然索引)。
 */
class SafInboxStore(
    private val face: DocumentsFace,
    private val grant: InboxGrantStore,
    /** 仪表化旁路(实机 logcat 单一过滤面;JVM 桩面 Log.i 无操作)。 */
    private val log: (String) -> Unit = { Log.i(TAG, it) },
    /**
     * 拷贝面注入缝:实机缺省 = [copyStagingInto](暂存终件 → 临时文档
     * "wt" 句柄的 8 MiB 流拷贝 + 等长对账);JVM 桩面无真 fd,单测注入
     * 伪拷贝钉死发布编排。抛异常 = 拷贝失败(publish 统一 SAF 删临时收口)。
     */
    private val copyInto: (treeUri: String, documentId: String, stagingPath: String, expectedSize: Long) -> Unit =
        { treeUri, docId, path, size -> copyStagingInto(face, treeUri, docId, path, size, log) },
) : InboxStore {

    // ── 在场查询(fs.query_dest/0x04,spec §7.4)──────────────────────────

    override fun exists(rel: String, size: Long): Boolean = try {
        existsInTree(rel, size)
    } catch (e: Exception) {
        // 面契约外异常兜网(实机面 ContentResolver 未归类形态):不在场收口
        log("在场:异常 rel=$rel (${e.javaClass.simpleName}: ${e.message})")
        false
    }

    private fun existsInTree(rel: String, size: Long): Boolean {
        // 消毒不过按不在场收口(false → daemon FULL 重收,安全方向)
        val placement = InboxPlacement.place(rel)
        if (placement == null) {
            log("在场:消毒不过 rel=$rel")
            return false
        }
        val treeUri = grant.treeUri()
        if (treeUri == null) {
            log("在场:未授权 rel=$rel")
            return false
        }
        val treeDocId = face.treeDocumentId(treeUri)
        if (treeDocId == null) {
            log("在场:树解析失败 rel=$rel")
            return false
        }
        // 收件根只查不建(纯查询不得有副作用;根未落位 = 一切终件均不在场)
        val inboxDocId = resolveInboxRoot(treeUri, treeDocId, rel, create = false)
        if (inboxDocId == null) {
            log("在场:收件根未落位 rel=$rel")
            return false
        }
        // rel 目录段下钻(drill 机器复用,与发送链同一注入缝);断链 → 不在场
        val dirs = rel.split('/').dropLast(1)
        val parentDocId = if (dirs.isEmpty()) {
            inboxDocId
        } else {
            SourceRefs.drillChildDocumentId(inboxDocId, dirs.joinToString("/")) { parent ->
                face.treeChildren(treeUri, parent)
            }
        }
        if (parentDocId == null) {
            log("在场:下钻断链 rel=$rel")
            return false
        }
        // 终判据:同名**文件**子文档真实存在且尺寸吻合(目录同名不算在场)
        val hit = face.treeChildren(treeUri, parentDocId)
            ?.firstOrNull { it.name == placement.displayName && !it.isDir }
        if (hit == null) {
            log("在场:无终件 rel=$rel")
            return false
        }
        val ok = hit.size == size
        log("在场:${if (ok) "命中" else "尺寸不符"} rel=$rel size=${hit.size} want=$size")
        return ok
    }

    // ── 发布(fs.publish/0x05,spec §7.3)──────────────────────────────────

    override fun publish(rel: String, stagingPath: String, size: Long, policy: TransferPolicy): PublishResult {
        // 消毒不过拒发布(与 daemon validate_rel_path 同口径的第二道防御)
        val placement = InboxPlacement.place(rel)
            ?: return PublishResult.Failed("非法相对路径(拒发布): $rel")
        // 授权闸:无授权/授权失效 → 诚实失败词面(任务「保存失败」,暂存
        // 保留;重发 republish 前在设置页完成授权,spec §7.1)
        val treeUri = grant.treeUri()
            ?: return PublishResult.Failed("收件箱未授权:请在 App 内完成目录授权后重试")
        val treeDocId = face.treeDocumentId(treeUri)
            ?: return PublishResult.Failed("收件箱授权无效(树 URI 解析失败): $rel")
        // 收件根归一化(spec §7.2):根名 = MTDSS 用根;否则树内 MTDSS
        // 子目录(存在则用,无则建)
        val inboxDocId = resolveInboxRoot(treeUri, treeDocId, rel, create = true)
            ?: return PublishResult.Failed("收件箱根定位失败: $rel")
        // rel 目录段逐级自建(目录结构保真,spec §5.1 语义沿袭)
        val parentDocId = ensureDirPath(treeUri, inboxDocId, rel.split('/').dropLast(1))
            ?: return PublishResult.Failed("收件目录创建失败: $rel")
        // ① 落位子项预查(spec §7.5):同名目录 → 拒(绝不递归删目录);
        // 真文件在场沿 policy(skip 弃件 / 其余 SAF 删后落位)
        val siblings = face.treeChildren(treeUri, parentDocId)
            ?: return PublishResult.Failed("落位子项查询失败: $rel")
        val clash = siblings.firstOrNull { it.name == placement.displayName }
        if (clash != null) {
            if (clash.isDir) {
                log("发布:目标被同名目录占用 rel=$rel")
                return PublishResult.Failed("目标路径被同名目录占用(请改名后重发): $rel")
            }
            if (policy.skips(rel)) {
                // skip 命中弃件:既有文件保留,暂存由 daemon 收尾清理
                // (0x05 ok:true);记录打标后 final_state 记录门+0x04 照常闭环
                log("发布:skip 命中弃件 rel=$rel docId=${clash.documentId}")
                return PublishResult.Skipped
            }
            // 驱逐(缺省 overwrite「落最新」/overwrite 命中同路)
            if (!face.deleteDocument(treeUri, clash.documentId)) {
                log("发布:驱逐失败 rel=$rel docId=${clash.documentId}")
                return PublishResult.Failed("落位驱逐失败: $rel")
            }
            log("发布:驱逐在场件 rel=$rel docId=${clash.documentId}")
        }
        // ② .mtdss 隐藏目录(存在则用;无则建——MIUI 点名改写双形态兼容,
        // spec §7.7)+ 点文件临时名建临时文档
        val stagingDirDocId = resolveStagingDir(treeUri, inboxDocId, rel)
            ?: return PublishResult.Failed(".mtdss 暂存目录定位失败: $rel")
        val tempName = tempDisplayName()
        val tempDocId = face.createDocument(treeUri, stagingDirDocId, tempName, mimeTypeOf(placement.displayName))
            ?: return PublishResult.Failed("临时文件创建失败: $rel")
        // ③ 流拷贝(8 MiB + 等长对账;失败 SAF 删临时——点文件在 .mtdss
        //    内,活进程失败零可见残留)
        try {
            copyInto(treeUri, tempDocId, stagingPath, size)
        } catch (e: Exception) {
            val cleaned = face.deleteDocument(treeUri, tempDocId)
            log("发布:拷贝失败删临时 rel=$rel docId=$tempDocId 删除=$cleaned (${e.javaClass.simpleName}: ${e.message})")
            return PublishResult.Failed("发布拷贝失败(rel=$rel): ${e.message}")
        }
        // ④ 移位落位父目录(rename 不换父目录,落位 = moveDocument 半步;
        //    以点文件临时名移动,随机名不撞终名;失败删临时)
        val movedDocId = face.moveDocument(treeUri, tempDocId, stagingDirDocId, parentDocId)
        if (movedDocId == null) {
            val cleaned = face.deleteDocument(treeUri, tempDocId)
            log("发布:移位失败删临时 rel=$rel docId=$tempDocId 删除=$cleaned")
            return PublishResult.Failed("发布落位失败(rel=$rel): 临时文件已清理,可重试")
        }
        // ⑤ 改名落正式名(rename 在落位父目录内进行):返回名 ≠ 请求名 =
        //    终名路径被占用而 provider 自动改 "(N)" → Failed 沿 F14b 词面
        //    (完整内容已落实际名,改名后的正式文件保留 = 行保留等价物;
        //    暂存未动,republish 可收拾)
        val renamed = try {
            face.renameDocument(treeUri, movedDocId, placement.displayName)
        } catch (e: Exception) {
            null
        }
        if (renamed == null) {
            // 改名义失败:件已在落位父目录内(点文件临时名,可见区不可见
            // 载体)——删除收口;旧临时 id(移动式已失效)一并尽力清
            val cleaned = face.deleteDocument(treeUri, movedDocId)
            if (movedDocId != tempDocId) face.deleteDocument(treeUri, tempDocId)
            log("发布:改名失败删件 rel=$rel docId=$movedDocId 删除=$cleaned")
            return PublishResult.Failed("发布改名失败(rel=$rel): 临时文件已清理,可重试")
        }
        // ⑥ 复制式变更残留清理(move/rename 是移动则旧 id 已失效,删除无害
        //    失败;复制式 provider 的同名旧件由此清掉)
        if (renamed.documentId != movedDocId) {
            val cleaned = face.deleteDocument(treeUri, movedDocId)
            log("发布:旧件清理 docId=$movedDocId 删除=$cleaned 新docId=${renamed.documentId}")
        }
        if (renamed.displayName != placement.displayName) {
            log("发布:改名碰撞 文件保留 rel=$rel want=${placement.displayName} actual=${renamed.displayName}")
            return PublishResult.Failed(
                "目标名被残留文件占用,完整内容已存为 '${renamed.displayName}'(可在文件管理器手动改名)",
            )
        }
        // ⑥ 索引触发(spec §7.6;推导不出路径交扫描器自然索引)
        scanPathOf(renamed.documentId)?.let { path ->
            face.scanFile(path)
            log("发布:扫描器已触发 rel=$rel path=$path")
        }
        log("发布:完成 rel=$rel docId=${renamed.documentId} size=$size")
        return PublishResult.Published
    }

    // ── 树定位(纯决策,face 注入,JVM 钉死)──────────────────────────────

    /**
     * 收件根归一化(spec §7.2):授权树根名 = MTDSS(忽略大小写)→ 根
     * 即收件箱;否则查树内 MTDSS 子目录(存在则用),[create] = true 且
     * 缺席时 createDirectory 自建。根名查询失败 → null(归一化不可判——
     * 盲建会在 MTDSS 根内再造 MTDSS/MTDSS 层级,诚实收口失败)。
     */
    private fun resolveInboxRoot(treeUri: String, treeDocId: String, rel: String, create: Boolean): String? {
        val rootName = face.documentName(treeUri, treeDocId)
        if (rootName == null) {
            log("收件根:根名查询失败 rel=$rel")
            return null
        }
        if (rootName.equals(INBOX_DIR_NAME, ignoreCase = true)) {
            return treeDocId
        }
        val children = face.treeChildren(treeUri, treeDocId)
        if (children == null) {
            log("收件根:根子项查询失败 rel=$rel")
            return null
        }
        val hit = children.firstOrNull { it.isDir && it.name.equals(INBOX_DIR_NAME, ignoreCase = true) }
        if (hit != null) return hit.documentId
        if (!create) {
            log("收件根:MTDSS 子目录未建(查询不创建)rel=$rel")
            return null
        }
        val created = face.createDirectory(treeUri, treeDocId, INBOX_DIR_NAME)
        log("收件根:自建 $INBOX_DIR_NAME docId=$created rel=$rel")
        return created
    }

    /**
     * `.mtdss` 暂存目录定位:同名双形态兼容(`.mtdss` 点名原形 / MIUI
     * 改写形 `_.mtdss`,spec §7.7——防改名形态下重复建目录);缺席则
     * createDirectory `.mtdss`(MIUI 可能再改写,仍收纳于专用目录)。
     */
    private fun resolveStagingDir(treeUri: String, inboxDocId: String, rel: String): String? {
        val children = face.treeChildren(treeUri, inboxDocId)
        if (children == null) {
            log("暂存目录:收件箱子项查询失败 rel=$rel")
            return null
        }
        val hit = children.firstOrNull { it.isDir && it.name in STAGING_DIR_NAMES }
        if (hit != null) return hit.documentId
        val created = face.createDirectory(treeUri, inboxDocId, STAGING_DIR_NAME)
        log("暂存目录:自建 $STAGING_DIR_NAME docId=$created rel=$rel")
        return created
    }

    /** rel 目录段逐级下钻定位/自建(段名精确匹配——目录结构保真)。 */
    private fun ensureDirPath(treeUri: String, inboxDocId: String, dirs: List<String>): String? {
        var docId = inboxDocId
        for (seg in dirs) {
            val children = face.treeChildren(treeUri, docId) ?: return null
            val hit = children.firstOrNull { it.isDir && it.name == seg }
            docId = hit?.documentId ?: face.createDirectory(treeUri, docId, seg) ?: return null
        }
        return docId
    }

    companion object {
        private const val TAG = "InboxProvider"

        /** 收件箱根目录名(授权树内归一化目标)。 */
        const val INBOX_DIR_NAME: String = "MTDSS"

        /** 发布暂存隐藏目录名(spec §7.3;MIUI 点名改写形态见 [STAGING_DIR_NAMES])。 */
        const val STAGING_DIR_NAME: String = ".mtdss"

        /** 暂存目录双形态(MIUI 若改写点名,`_.mtdss` 形同样命中——防重复建目录)。 */
        val STAGING_DIR_NAMES: Set<String> = setOf(".mtdss", "_.mtdss")

        /** 发布拷贝缓冲:8 MiB(终件可达 GB 级,大缓冲降拷贝循环开销)。 */
        private const val COPY_BUFFER_BYTES: Int = 8 * 1024 * 1024

        /** 点文件临时名前缀/后缀(形态 JVM 钉死:dot 开头 + .tmp 收尾)。 */
        private const val TEMP_NAME_PREFIX = ".mtdss-pub-"
        private const val TEMP_NAME_SUFFIX = ".tmp"

        /**
         * 点文件临时名:`.mtdss-pub-<8hex>.tmp`(uuid 每次新)。dotfile 落
         * `.mtdss` 隐藏目录——强杀残留的半拷贝对用户/相册/扫描器不可见;
         * 随机名恒异于终名与彼此,不占终名路径、不成碰撞源。
         */
        fun tempDisplayName(): String =
            TEMP_NAME_PREFIX + UUID.randomUUID().toString().replace("-", "").take(8) + TEMP_NAME_SUFFIX

        /** 按扩展名猜 MIME;未知兜 application/octet-stream(getSingleton 用
         *  `?.`——实机恒非 null 行为不变,JVM 桩面不 NPE)。 */
        fun mimeTypeOf(name: String): String {
            val ext = name.substringAfterLast('.', "").lowercase()
            if (ext.isEmpty()) return "application/octet-stream"
            return MimeTypeMap.getSingleton()?.getMimeTypeFromExtension(ext)
                ?: "application/octet-stream"
        }

        /**
         * rename 后的扫描路径推导(ExternalStorageProvider docId 形
         * `primary:Download/MTDSS/…` → `/storage/emulated/0/Download/…`);
         * 非主卷/非路径形 docId(纯数字等)→ null(交扫描器自然索引)。
         */
        fun scanPathOf(documentId: String): String? {
            val idx = documentId.indexOf(':')
            if (idx <= 0 || documentId.substring(0, idx) != "primary") return null
            val rest = documentId.substring(idx + 1)
            if (rest.isEmpty() || rest.startsWith("/")) return null
            return "/storage/emulated/0/$rest"
        }

        /**
         * 发布拷贝(实机缺省拷贝面):暂存终件 → 临时文档 "wt" 句柄的流拷贝
         * (8 MiB 缓冲),实拷字节数对账 [expectedSize](暂存件已过 finalize
         * 哈希,拷贝即等长;不符 = 读到残件,按失败收口防假发布)。写句柄
         * 生命周期交 AutoCloseOutputStream 托管(use 关流即关文档句柄,恰一
         * 次);暂存源独立 use——源缺席/不可读时 FileInputStream 构造先抛
         * (JVM 单测由此钉住「拷贝失败删临时」的真实路径)。任何失败抛异常
         * (publish 统一 SAF 删临时收口)。
         */
        fun copyStagingInto(
            face: DocumentsFace,
            treeUri: String,
            documentId: String,
            stagingPath: String,
            expectedSize: Long,
            log: (String) -> Unit = {},
        ) {
            android.os.ParcelFileDescriptor.AutoCloseOutputStream(
                face.openWrite(treeUri, documentId),
            ).use { out ->
                FileInputStream(stagingPath).use { input ->
                    var copied = 0L
                    val buf = ByteArray(COPY_BUFFER_BYTES)
                    while (true) {
                        val n = input.read(buf)
                        if (n < 0) break
                        out.write(buf, 0, n)
                        copied += n
                    }
                    if (copied != expectedSize) {
                        throw IOException("拷贝字节数不符:实拷 $copied 预期 $expectedSize")
                    }
                    log("发布:拷贝完成 docId=$documentId 字节=$copied")
                }
            }
        }
    }
}
