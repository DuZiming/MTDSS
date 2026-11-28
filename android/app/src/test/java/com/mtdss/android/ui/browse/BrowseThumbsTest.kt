package com.mtdss.android.ui.browse

import com.mtdss.android.ipc.DirEntry
import com.mtdss.android.ipc.PreviewOpenResult
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.fail
import org.junit.Test

/**
 * 浏览缩略图纯函数面 + 句柄池 JVM 单测(V4-1 T22;钦定 mockup mobile-tiles-v2
 * / mobile-browse.html):
 * - **缩略图资格矩阵**:图片/视频扩展名(词表与桌面 ui/src/lib/filetype.ts
 *   img/vid 同源,大小写不敏感);目录/无扩展名/点文件/未知扩展名 → 图标占位;
 * - **URL 组装**:`http://127.0.0.1:{port}/f/{handle}/thumb?w=256&t={token}`
 *   形逐字(?t= 查询令牌与 Bearer 双缝 T20 面等价;样本值 = daemon 契约
 *   golden 同源);
 * - **时长角标格式化**(桌面 ui/src/lib/duration.ts fmtDuration 逐字镜像):
 *   m:ss / h:mm:ss;非正/非数 → 空串(有则显无则不显);
 * - **句柄池生命周期**:引用计数(在场 open,离场 close——IPC 无批量 op,
 *   逐条目);同路径复用单 handle;归零才 close;open 失败 = null 零 close;
 *   releaseAll 收口;在途 open 遇离场取消 = 不留槽(泄漏闭合,daemon 60s
 *   空闲回收为终极兜底)。
 */
class BrowseThumbsTest {

    private fun entry(name: String, isDir: Boolean = false, secs: Double? = null) =
        DirEntry(name = name, path = "/$name", size = 1, mtimeMs = 0, isDir = isDir, durationSecs = secs)

    /** 样本凭据(daemon 契约 golden preview_open.resp 同源值)。 */
    private val openResult = PreviewOpenResult(
        handle = "prev-fixed-0000000000000001",
        port = 39801,
        token = "prev-fixed-token",
    )

    // ── 缩略图资格矩阵 ─────────────────────────────────────────────────────

    @Test
    fun `图片扩展名资格 词表与桌面 img 同源`() {
        for (ext in listOf("jpg", "jpeg", "png", "gif", "bmp", "webp", "heic", "svg", "tif", "tiff", "ico", "avif")) {
            assertEquals(
                "图片资格: .$ext",
                true,
                BrowseThumbs.thumbEligible("横版照片.$ext", isDir = false),
            )
            assertEquals(
                "大小写不敏感: .$ext",
                true,
                BrowseThumbs.thumbEligible("IMG_2041.${ext.uppercase()}", isDir = false),
            )
        }
    }

    @Test
    fun `视频扩展名资格 词表与桌面 vid 同源`() {
        for (ext in listOf("mp4", "mkv", "mov", "avi", "webm", "m4v", "wmv", "flv", "mpg", "mpeg", "3gp")) {
            assertEquals(
                "视频资格: .$ext",
                true,
                BrowseThumbs.thumbEligible("VID_0032.$ext", isDir = false),
            )
        }
    }

    @Test
    fun `目录与非常规文件名一律图标占位`() {
        assertEquals(false, BrowseThumbs.thumbEligible("Screenshots", isDir = true)) // 目录恒图标
        assertEquals(false, BrowseThumbs.thumbEligible("Screenshots", isDir = false)) // 无扩展名
        assertEquals(false, BrowseThumbs.thumbEligible(".env", isDir = false)) // 点文件
        assertEquals(false, BrowseThumbs.thumbEligible("readme.", isDir = false)) // 点在末尾
        assertEquals(false, BrowseThumbs.thumbEligible("合同_v3.docx", isDir = false)) // 文档
        assertEquals(false, BrowseThumbs.thumbEligible("素材包.zip", isDir = false))
        assertEquals(false, BrowseThumbs.thumbEligible("noext", isDir = false))
    }

    // ── URL 组装 ───────────────────────────────────────────────────────────

    @Test
    fun `thumb URL 形逐字 含 w 与 t 查询令牌`() {
        assertEquals(
            "http://127.0.0.1:39801/f/prev-fixed-0000000000000001/thumb?w=256&t=prev-fixed-token",
            BrowseThumbs.thumbUrl(39801, "prev-fixed-0000000000000001", "prev-fixed-token"),
        )
    }

    @Test
    fun `thumb URL 自定宽透传 缺省 256`() {
        assertEquals(
            "http://127.0.0.1:80/f/h/thumb?w=512&t=tk",
            BrowseThumbs.thumbUrl(80, "h", "tk", w = 512),
        )
        assertEquals(256, BrowseThumbs.THUMB_WIDTH)
    }

    // ── 时长角标 ───────────────────────────────────────────────────────────

    @Test
    fun `时长角标 m-ss 与 h-mm-ss 形 桌面 fmtDuration 逐字同构`() {
        assertEquals("0:59", BrowseThumbs.durationBadge(59.9)) // 小数向下取整
        assertEquals("1:31", BrowseThumbs.durationBadge(91.5)) // daemon golden 值
        assertEquals("3:12", BrowseThumbs.durationBadge(192.0)) // mockup 角标值
        assertEquals("1:00:00", BrowseThumbs.durationBadge(3600.0))
        assertEquals("1:01:01", BrowseThumbs.durationBadge(3661.0))
        assertEquals("12:34:56", BrowseThumbs.durationBadge(45296.0))
    }

    @Test
    fun `时长角标 未知零负非数一律空串`() {
        assertNull(entry("v.mp4").durationSecs)
        assertEquals("", BrowseThumbs.durationBadge(null))
        assertEquals("", BrowseThumbs.durationBadge(0.0))
        assertEquals("", BrowseThumbs.durationBadge(-3.5))
        assertEquals("", BrowseThumbs.durationBadge(Double.NaN))
        assertEquals("", BrowseThumbs.durationBadge(Double.POSITIVE_INFINITY))
    }

    @Test
    fun `角标只落视频名 图片有 secs 也不显`() {
        assertEquals("1:31", BrowseThumbs.durationBadgeFor("IMG_2041.mp4", 91.5))
        assertEquals("", BrowseThumbs.durationBadgeFor("横版照片.jpg", 91.5))
        assertEquals("", BrowseThumbs.durationBadgeFor("IMG_2041.mp4", 0.0))
        assertEquals("", BrowseThumbs.durationBadgeFor("IMG_2041.mp4", null))
    }

    // ── 句柄池生命周期 ─────────────────────────────────────────────────────

    @Test
    fun `同路径引用计数 单次 open 归零才 close`() = runBlocking {
        var opens = 0
        var closes = 0
        val pool = PreviewHandlePool(
            open = { opens++; Result.success(openResult) },
            close = { closes++ },
        )
        val e = entry("a.jpg")
        val u1 = pool.urlFor(e)!!
        val u2 = pool.urlFor(e)!!
        assertEquals(1, opens) // 池内复用,不重复 open(daemon 去重之外的本地面)
        assertEquals(u1, u2)
        assertEquals(1, pool.liveCount)
        pool.release(e)
        assertEquals(0, closes) // 尚有余量,不 close
        assertEquals(1, pool.liveCount)
        pool.release(e)
        assertEquals(1, closes) // 归零即 close
        assertEquals(0, pool.liveCount)
    }

    @Test
    fun `URL 携该槽自己的 port token`() = runBlocking {
        val pool = PreviewHandlePool(
            open = { Result.success(openResult) },
            close = {},
        )
        assertEquals(
            BrowseThumbs.thumbUrl(39801, "prev-fixed-0000000000000001", "prev-fixed-token"),
            pool.urlFor(entry("a.jpg")),
        )
    }

    @Test
    fun `open 失败返回 null 且零 close`() = runBlocking {
        val pool = PreviewHandlePool(
            open = { Result.failure(IllegalStateException("源打开失败")) },
            close = { fail("open 失败不应触发 close") },
        )
        assertNull(pool.urlFor(entry("a.jpg")))
        assertEquals(0, pool.liveCount)
    }

    @Test
    fun `release 未知路径幂等不炸`() = runBlocking {
        var closes = 0
        val pool = PreviewHandlePool(open = { Result.success(openResult) }, close = { closes++ })
        pool.release(entry("ghost.jpg"))
        assertEquals(0, closes)
    }

    @Test
    fun `release 后再取 重新 open 新槽`() = runBlocking {
        var opens = 0
        var closes = 0
        val pool = PreviewHandlePool(
            open = { opens++; Result.success(openResult) },
            close = { closes++ },
        )
        val e = entry("a.jpg")
        pool.urlFor(e)
        pool.release(e)
        pool.urlFor(e)
        assertEquals(2, opens)
        assertEquals(1, closes)
        assertEquals(1, pool.liveCount)
    }

    @Test
    fun `releaseAll 收口全槽`() = runBlocking {
        var closes = 0
        val pool = PreviewHandlePool(open = { Result.success(openResult) }, close = { closes++ })
        pool.urlFor(entry("a.jpg"))
        pool.urlFor(entry("b.png"))
        pool.urlFor(entry("c.mp4"))
        pool.releaseAll()
        assertEquals(3, closes)
        assertEquals(0, pool.liveCount)
    }

    @Test
    fun `close 失败静默 不炸调用方`() = runBlocking {
        val pool = PreviewHandlePool(
            open = { Result.success(openResult) },
            close = { throw IllegalStateException("ipc 断开") },
        )
        val e = entry("a.jpg")
        pool.urlFor(e)
        pool.release(e) // close 抛错被池吞掉(daemon 60s 空闲回收兜底)
        assertEquals(0, pool.liveCount)
    }

    @Test
    fun `在途 open 遇离场取消 不留槽零 close`() = runBlocking {
        var entered = false
        var closes = 0
        val pool = PreviewHandlePool(
            open = {
                entered = true
                delay(5_000) // 模拟 IPC 在途
                Result.success(openResult)
            },
            close = { closes++ },
        )
        val job = launch { pool.urlFor(entry("a.jpg")) }
        while (!entered) delay(5) // 等 open 进入在途
        job.cancelAndJoin() // 行离场 = LaunchedEffect 取消
        assertEquals(0, closes) // 取消路径不触发 close(无槽可关)
        assertEquals(0, pool.liveCount) // 不留半开槽
    }
}
