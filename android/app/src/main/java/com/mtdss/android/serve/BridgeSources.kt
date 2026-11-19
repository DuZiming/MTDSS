package com.mtdss.android.serve

import android.util.Log
import com.mtdss.android.send.OpenedSource
import com.mtdss.android.send.RootLookup
import com.mtdss.android.send.SafFace
import com.mtdss.android.send.SourceRefs

/**
 * 桥源开档共用单点(V4-1 T13 提取;seek/thumb/rendition 三桥同走):根
 * 解析单点([resolveServeRoot])之上的「父列表定位终段 + 照片批内唯一名
 * 重放」行走面。各桥自实现必漂移(V3-4 根解析提共享单点同教训——
 * [photoVpathSub] 同轨先例),提取后三桥共用一份。
 *
 * 词面参数 [what] = 各桥动词("seek"/"出图"/"转码"),错误词跨桥同构仅
 * 动词异(既有桥测试的词面断言兼容:含「目录」「照片」「源已不存在」等
 * 稳定子串不变)。
 */
internal object BridgeSources {

    private const val TAG = "BridgeSources"

    /** 开档结果:fd 源 + 显示名(桥侧分型凭据/日志面)。 */
    class BridgeSource(val opened: OpenedSource, val displayName: String)

    /**
     * vpath → 可 seek 只读源。`/照片图库` 前缀 = MediaStore 臂,其余 =
     * SAF 服务桥根面;根自身/目录/缺席/未授权一律失败词面(调用方按各自
     * 桥语义收口:seek/thumb 拒答回退、rendition 拒答 unsupported)。阻塞
     * 面,调用方在 IO 线程。
     */
    fun open(
        path: String,
        what: String,
        saf: SafFace,
        inboxGrant: InboxGrantLookup,
        roots: RootLookup,
        photos: PhotoLibraryFace,
    ): Result<BridgeSource> {
        val sub = photoVpathSub(path)
        return if (sub != null) {
            openPhoto(sub, what, photos)
        } else {
            openSaf(path, what, saf, inboxGrant, roots)
        }
    }

    // ── MediaStore 臂(照片 vpath)────────────────────────────────────────

    /**
     * 照片臂:`相册/名` → 相册照片批内唯一名重放(与 daemon 照片树同序
     * ——[PhotoResponder.uniqueName] 原始名优先、撞名 ` (n)` 顺延)解析
     * photoId → [PhotoLibraryFace.openSeekable]。根("")/无文件段/相册缺席/
     * 名缺席/资产不可开 = 失败词面。
     */
    private fun openPhoto(
        sub: String,
        what: String,
        photos: PhotoLibraryFace,
    ): Result<BridgeSource> {
        if (sub.isEmpty()) {
            return Result.failure(IllegalStateException("照片图库根不可 $what(仅文件)"))
        }
        val album = sub.substringBefore('/')
        val name = sub.substringAfter('/', missingDelimiterValue = "")
        if (name.isEmpty()) {
            return Result.failure(IllegalStateException("相册不可 $what(仅文件): $album"))
        }
        val assets = photos.photos(album)
            ?: return Result.failure(IllegalStateException("无此相册: $album"))
        // 批内唯一名重放(listEntries 同序同函数——daemon 侧树名即此处名)
        val used = HashSet<String>()
        for (asset in assets) {
            val unique = PhotoResponder.uniqueName(asset.name, used)
            if (unique == name) {
                val opened = photos.openSeekable(asset.photoId)
                    ?: return Result.failure(IllegalStateException("资产不可用: ${asset.photoId}"))
                return Result.success(BridgeSource(opened, asset.name))
            }
        }
        return Result.failure(IllegalStateException("源已不存在: $album/$name"))
    }

    // ── SAF 臂(服务桥根 vpath)────────────────────────────────────────────

    /**
     * SAF 臂:`/根/子/名` → 根面解析(收件箱/授权根,[resolveServeRoot]
     * 共享单点)→ 父列表定位终段(条目 isDir 由此判,DocMeta 无目录旗)→
     * 只读句柄。根自身/目录/缺席 = 失败词面。
     */
    private fun openSaf(
        path: String,
        what: String,
        saf: SafFace,
        inboxGrant: InboxGrantLookup,
        roots: RootLookup,
    ): Result<BridgeSource> {
        // 绝对 vpath 切根:首段 = 根(收件箱/授权根 vpath),余 = 子路径
        // (daemon bridge_route 命中形 = root + "/" + sub,App 侧逆切同口径)
        val body = path.removePrefix("/")
        val root = "/" + body.substringBefore('/', missingDelimiterValue = body)
        val sub = body.substringAfter('/', missingDelimiterValue = "")
        val tree = when (val resolved = resolveServeRoot(root, saf, inboxGrant, roots)) {
            is ServeRootResolved.Ok -> resolved
            is ServeRootResolved.Refused -> return Result.failure(IllegalStateException(resolved.reason))
        }
        if (!serveSubOk(sub)) return Result.failure(IllegalStateException("非法子路径: $path"))
        if (sub.isEmpty()) {
            return Result.failure(IllegalStateException("根不可 $what(仅文件): $root"))
        }
        // 父列表定位终段(条目 isDir 判据来自列表条目)
        val name = sub.substringAfterLast('/')
        val parentSub = sub.substringBeforeLast('/', missingDelimiterValue = "")
        val parentDoc = if (parentSub.isEmpty()) {
            tree.baseDocId
        } else {
            SourceRefs.drillChildDocumentId(tree.baseDocId, parentSub) { parent ->
                saf.treeChildren(tree.treeUri, parent)
            } ?: return Result.failure(IllegalStateException("源已不存在: $root/$parentSub"))
        }
        val hit = saf.treeChildren(tree.treeUri, parentDoc)?.firstOrNull { it.name == name }
            ?: return Result.failure(IllegalStateException("源已不存在: $path"))
        if (hit.isDir) {
            return Result.failure(IllegalStateException("目录不可 $what(仅文件): $path"))
        }
        return runCatching { saf.openRead(saf.buildChildUri(tree.treeUri, hit.documentId)) }
            .map { opened -> BridgeSource(opened, hit.name) }
            .onFailure { Log.w(TAG, "SAF $what 打开失败 $path: ${it.message}") }
    }
}
