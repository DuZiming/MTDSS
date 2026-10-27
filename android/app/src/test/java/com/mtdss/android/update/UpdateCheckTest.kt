package com.mtdss.android.update

import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asObject
import com.mtdss.android.ipc.toCanonicalJson
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 更新清单核验 JVM 单测(M4 镜像面):canonical 字节与 crates/core/src/update.rs
 * 的 golden 字符串逐字节锁定(Rust 测试 canonical_bytes_deterministic_and_sorted
 * 同样本);验签/篡改/编码失败/公钥面与版本比较逐例镜像 Rust 测试
 * (sign_verify_roundtrip / tamper_any_signed_field_breaks_verify /
 * signature_tamper_and_encoding_failures / version_cmp_*)。
 */
class UpdateCheckTest {

    // 与 Rust 侧测试同值(64 hex 字符;长度断言防手写错位)
    private val aa64 = "a".repeat(64)
    private val bb64 = "b".repeat(64)

    /** Rust sample_manifest 同构;windows 键故意放前——canonical 写出按字母序,插入序不得影响字节。 */
    private fun sampleManifest(): UpdateManifest = UpdateManifest(
        version = "1.2.3",
        notes = "bug fixes",
        pubdate = "2026-10-21T00:00:00Z",
        platforms = linkedMapOf(
            "windows-x86_64" to PlatformAsset(
                url = "https://dl.example.invalid/win/1.2.3/setup.exe",
                sha256 = aa64,
                size = 123_456,
            ),
            "linux-x86_64" to PlatformAsset(
                url = "https://dl.example.invalid/linux/1.2.3/app.tar.gz",
                sha256 = bb64,
                size = 4_242,
            ),
        ),
        signature = "",
    )

    private fun signed(seed: Byte): UpdateManifest = UpdateFixtures.signed(sampleManifest(), seed)

    @Test
    fun `hex 常量长度为 64`() {
        assertEquals(64, aa64.length)
        assertEquals(64, bb64.length)
    }

    @Test
    fun `canonical bytes 与 mtdss_core golden 逐字节一致`() {
        // Rust 侧白板 JSON:紧凑、全对象键字母序(顶层 notes < platforms <
        // pubdate < version;平台 linux < windows;asset sha256 < size < url)
        val expected = """{"notes":"bug fixes","platforms":{""" +
            """"linux-x86_64":{"sha256":"$bb64","size":4242,"url":"https://dl.example.invalid/linux/1.2.3/app.tar.gz"},""" +
            """"windows-x86_64":{"sha256":"$aa64","size":123456,"url":"https://dl.example.invalid/win/1.2.3/setup.exe"}},""" +
            """"pubdate":"2026-10-21T00:00:00Z","version":"1.2.3"}"""
        assertEquals(expected, String(signed(1).canonicalBytes(), Charsets.UTF_8))
    }

    @Test
    fun `canonical bytes 与 signature 值无关`() {
        assertEquals(sampleManifest().canonicalBytes().toList(), signed(1).canonicalBytes().toList())
        val altSig = sampleManifest().copy(signature = "deadbeef")
        assertEquals(signed(1).canonicalBytes().toList(), altSig.canonicalBytes().toList())
    }

    @Test
    fun `parse 往返等值`() {
        val m = signed(1)
        assertEquals(m, UpdateCheck.parseManifest(UpdateFixtures.manifestJson(m)))
        // 签名后 JSON 里的 size 数值形态(十进制原文)不影响回读
        assertEquals(123_456L, UpdateCheck.parseManifest(UpdateFixtures.manifestJson(m))
            .platforms.getValue("windows-x86_64").size)
    }

    @Test
    fun `parse 严格镜像 serde 必填面`() {
        val m = signed(1)
        for (key in listOf("version", "notes", "pubdate", "platforms", "signature")) {
            val obj = JVal.parse(UpdateFixtures.manifestJson(m)).asObject()
            obj.entries.remove(key)
            assertThrows(IllegalArgumentException::class.java) {
                UpdateCheck.parseManifest(obj.toCanonicalJson())
            }
        }
        // asset 内字段缺失/类型错(serde missing field / invalid type 同形拒绝)
        val badAsset = JVal.Obj(
            "url" to JVal.str("https://dl.example.invalid/x.apk"),
            "size" to JVal.num(1),
        )
        val badPlatforms = JVal.Obj(ANDROID_PLATFORM_KEY to badAsset)
        for (bad in listOf(
            JVal.Obj("url" to JVal.str("u"), "sha256" to JVal.str(aa64)), // 缺 size
            badAsset,
            JVal.Obj(
                "url" to JVal.str("u"),
                "sha256" to JVal.str(aa64),
                "size" to JVal.str("123"), // size 非整数
            ),
        )) {
            val obj = JVal.parse(UpdateFixtures.manifestJson(m)).asObject()
            obj.entries["platforms"] = JVal.Obj(ANDROID_PLATFORM_KEY to bad)
            assertThrows(IllegalArgumentException::class.java) {
                UpdateCheck.parseManifest(obj.toCanonicalJson())
            }
        }
        // 空 platforms 映射合法(serde 无 min_items;资产查找留给消费方)
        val obj = JVal.parse(UpdateFixtures.manifestJson(m)).asObject()
        obj.entries["platforms"] = JVal.Obj()
        assertEquals(0, UpdateCheck.parseManifest(obj.toCanonicalJson()).platforms.size)
    }

    @Test
    fun `parse 忽略未知字段(镜像 serde 默认)`() {
        val obj = JVal.parse(UpdateFixtures.manifestJson(signed(1))).asObject()
        obj.entries["extra"] = JVal.str("ignored")
        assertEquals(signed(1), UpdateCheck.parseManifest(obj.toCanonicalJson()))
    }

    @Test
    fun `签名往返`() {
        assertTrue(UpdateCheck.verifySignature(signed(1), UpdateFixtures.pubHex(1)))
    }

    @Test
    fun `篡改任一被签字段即验签失败`() {
        val tamper: List<(UpdateManifest) -> UpdateManifest> = listOf(
            { it.copy(version = "9.9.9") },
            { it.copy(notes = "malicious") },
            { it.copy(pubdate = "1999-01-01T00:00:00Z") },
            { it.copy(platforms = it.platforms + ("windows-x86_64" to it.platforms.getValue("windows-x86_64").copy(url = "https://evil.invalid/setup.exe"))) },
            { it.copy(platforms = it.platforms + ("windows-x86_64" to it.platforms.getValue("windows-x86_64").copy(sha256 = aa64.replace('a', 'c')))) },
            { it.copy(platforms = it.platforms + ("linux-x86_64" to it.platforms.getValue("linux-x86_64").copy(size = 4_243))) },
        )
        for ((i, t) in tamper.withIndex()) {
            assertFalse("篡改用例 $i 不应验签通过", UpdateCheck.verifySignature(t(signed(1)), UpdateFixtures.pubHex(1)))
        }
    }

    @Test
    fun `签名编码面 fail-closed`() {
        val vk1 = UpdateFixtures.pubHex(1)
        val m = signed(1)
        // 非 hex / 空 / 32B / 65B / 奇数长度 / 合法编码但不匹配 → 全 false
        for (sig in listOf(
            "z".repeat(64),
            "",
            UpdateFixtures.hex(ByteArray(32)),
            UpdateFixtures.hex(ByteArray(65)),
            "a".repeat(63),
            UpdateFixtures.hex(ByteArray(64)),
        )) {
            assertFalse(UpdateCheck.verifySignature(m.copy(signature = sig), vk1))
        }
        // 错误密钥(镜像 wrong_vk)
        assertFalse(UpdateCheck.verifySignature(m, UpdateFixtures.pubHex(2)))
        // 合法签名本体(对照,防上面全 false 掩盖真失败)
        assertTrue(UpdateCheck.verifySignature(m, vk1))
    }

    @Test
    fun `公钥非法 fail-closed`() {
        val m = signed(1)
        for (pub in listOf("", "zz".repeat(32), "a".repeat(63), UpdateFixtures.hex(ByteArray(31)))) {
            assertFalse(UpdateCheck.verifySignature(m, pub))
        }
        assertNull(UpdateCheck.pubkeyBytes("not-hex"))
        assertNull(UpdateCheck.pubkeyBytes(""))
        assertEquals(32, UpdateCheck.pubkeyBytes(UpdateFixtures.pubHex(1))!!.size)
    }

    @Test
    fun `hex 解码大小写与奇偶面`() {
        assertEquals(1, UpdateCheck.hexToBytes("aB")!!.size)
        assertNull(UpdateCheck.hexToBytes("abc"))
        assertNull(UpdateCheck.hexToBytes("g0"))
        // 大写公钥/签名验签同过(hex::decode 双大小写口径)
        val m = signed(1)
        val upper = UpdateCheck.verifySignature(
            m.copy(signature = m.signature.uppercase()),
            UpdateFixtures.pubHex(1).uppercase(),
        )
        assertTrue(upper)
    }

    @Test
    fun `versionCompare 数值序而非字典序`() {
        assertEquals(-1, UpdateCheck.versionCompare("1.0.0", "1.0.1"))
        assertEquals(-1, UpdateCheck.versionCompare("1.2.0", "1.10.0")) // 字典序陷阱
        assertEquals(1, UpdateCheck.versionCompare("1.10.0", "1.2.0"))
        assertEquals(1, UpdateCheck.versionCompare("2.0.0", "1.99.99"))
        assertEquals(0, UpdateCheck.versionCompare("1.2.3", "1.2.3"))
        assertEquals(0, UpdateCheck.versionCompare("0.0.0", "0.0.0"))
    }

    @Test
    fun `versionCompare 拒绝非严格 XYZ`() {
        for (bad in listOf(
            "1.0", "1.0.0.0", "1.0.0-rc1", "v1.0.0", "1..0", ".1.0", "1.0.", "",
            "1.0.x", " 1.0.0", "1.0.0 ", "1.0.0\n", "+1.0.0", "99999999999999999999.0.0",
        )) {
            assertNull("a=$bad 必须拒绝", UpdateCheck.versionCompare(bad, "1.0.0"))
            assertNull("b=$bad 必须拒绝", UpdateCheck.versionCompare("1.0.0", bad))
        }
    }
}
