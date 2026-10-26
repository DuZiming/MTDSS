package com.mtdss.android.update

import com.mtdss.android.ipc.ContractViolationException
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.asObject
import com.mtdss.android.ipc.asString
import com.mtdss.android.ipc.opt
import com.mtdss.android.ipc.toCanonicalJson
import org.bouncycastle.crypto.params.Ed25519PublicKeyParameters
import org.bouncycastle.crypto.signers.Ed25519Signer

/**
 * 签名更新清单 —— M4 信任链的安卓镜像(spec §6「应用内更新」)。
 *
 * 与 crates/core/src/update.rs **逐字节同源**(同一枚 ed25519 密钥签的清单
 * 在 Windows daemon 与安卓 App 两端同口径验签,不设第二套语义):
 * - canonical 域 = 剔除 signature 后的紧凑 JSON、全对象键字母序(serde_json
 *   走 Value ⇒ BTreeMap ⇒ 键恒字典序的口径;Kotlin 侧复用 ipc/Json.kt 的
 *   [toCanonicalJson],其键序/转义面已被 IPC 契约 golden 逐字节锁定)。
 *   Rust 侧 golden 字符串在 UpdateCheckTest 与本侧逐字节比对;
 * - 验签 fail-closed:hex 解码失败 / 签名长度 != 64 / 密钥不符 / 公钥非法
 *   一律 false,绝不抛出(镜像 UpdateManifest::verify);
 * - 版本比较:严格 X.Y.Z 三段数值序,一切非法输入 null(镜像 version_cmp;
 *   注意 1.2.0 < 1.10.0 的数值序陷阱,字典序会判反);
 * - 解析:必填面/类型面严格(镜像 serde Deserialize 的 missing field /
 *   invalid type 拒绝),未知字段忽略(serde 默认),platforms 允许空映射
 *   (资产查找留给消费方——镜像 M4 在 apply 才查 PLATFORM_KEY)。
 *
 * ## ed25519 实现选型(T12 裁定)
 * java.security 的 Ed25519 到 **API 33** 才可用,本 App minSdk 29(MIX 3 =
 * Android 10 是验证下限)拿不到——打包 BouncyCastle bcprov-jdk18on,只走
 * 轻量 API(org.bouncycastle.crypto.*),不注册 JCA provider(注册面会与
 * 系统裁剪版 com.android.org.bouncycastle 冲突,轻量 API 无此问题)。
 * 与 mtdss-sign 的 ed25519-dalek 互操作由同一 canonical 字节 + RFC 8032
 * 标准曲线保证。
 */

/** 安卓资产在清单 platforms 映射里的键(对应 daemon 侧 "windows-x86_64" 键形)。 */
const val ANDROID_PLATFORM_KEY: String = "android-arm64"

/** 单平台资产(镜像 mtdss_core::update::PlatformAsset)。 */
data class PlatformAsset(
    val url: String,
    val sha256: String,
    val size: Long,
)

/** 签名更新清单(镜像 mtdss_core::update::UpdateManifest;signature = hex(ed25519))。 */
data class UpdateManifest(
    val version: String,
    val notes: String,
    val pubdate: String,
    val platforms: Map<String, PlatformAsset>,
    val signature: String,
) {
    /** 剔除 signature 的 JSON 值(键序无关;canonical 写出时统一字母序)。 */
    fun strippedValue(): JVal = JVal.Obj(
        "version" to JVal.str(version),
        "notes" to JVal.str(notes),
        "pubdate" to JVal.str(pubdate),
        "platforms" to JVal.Obj(
            LinkedHashMap<String, JVal>().apply {
                platforms.forEach { (k, a) ->
                    put(
                        k,
                        JVal.Obj(
                            "url" to JVal.str(a.url),
                            "sha256" to JVal.str(a.sha256),
                            "size" to JVal.num(a.size),
                        ),
                    )
                }
            },
        ),
    )

    /** canonical 字节:与 mtdss_core::update::UpdateManifest::canonical_bytes 逐字节一致。 */
    fun canonicalBytes(): ByteArray = strippedValue().toCanonicalJson().toByteArray(Charsets.UTF_8)
}

object UpdateCheck {

    /** 解析清单;缺字段/类型不符/JSON 非法 → IllegalArgumentException(调用方按操作失败收)。 */
    fun parseManifest(json: String): UpdateManifest = try {
        fromValue(JVal.parse(json))
    } catch (e: ContractViolationException) {
        throw IllegalArgumentException("清单字段形态不符: ${e.message}")
    } catch (e: IllegalArgumentException) {
        throw IllegalArgumentException("清单非法: ${e.message}")
    }

    private fun fromValue(root: JVal): UpdateManifest {
        // 局部助手:必填字段缺失即拒(镜像 serde missing field);未知字段忽略
        fun field(container: JVal?, key: String): JVal =
            container?.opt(key) ?: throw IllegalArgumentException("缺少字段: $key")

        val platforms = LinkedHashMap<String, PlatformAsset>()
        for ((key, value) in field(root, "platforms").asObject().entries) {
            val asset = value.asObject()
            platforms[key] = PlatformAsset(
                url = field(asset, "url").asString(),
                sha256 = field(asset, "sha256").asString(),
                size = field(asset, "size").asLong(),
            )
        }
        return UpdateManifest(
            version = field(root, "version").asString(),
            notes = field(root, "notes").asString(),
            pubdate = field(root, "pubdate").asString(),
            platforms = platforms,
            signature = field(root, "signature").asString(),
        )
    }

    /** 验签:canonical 字节 + hex 签名;fail-closed,任何输入缺陷/库异常 → false。 */
    fun verifySignature(m: UpdateManifest, pubkeyHex: String): Boolean {
        val pub = pubkeyBytes(pubkeyHex) ?: return false
        val sig = hexToBytes(m.signature) ?: return false
        if (sig.size != 64) return false
        return try {
            val signer = Ed25519Signer()
            signer.init(false, Ed25519PublicKeyParameters(pub, 0))
            val bytes = m.canonicalBytes()
            signer.update(bytes, 0, bytes.size)
            signer.verifySignature(sig)
        } catch (_: Exception) {
            false
        }
    }

    /** 验签公钥 hex → 32 字节;非法(非 hex/长度错)→ null(调用方判「未配置」)。 */
    fun pubkeyBytes(hex: String): ByteArray? {
        val bytes = hexToBytes(hex) ?: return null
        return if (bytes.size == 32) bytes else null
    }

    /** hex 解码(大小写均可,镜像 hex::decode;奇数长度/非 hex 字符 → null)。 */
    fun hexToBytes(hex: String): ByteArray? {
        if (hex.length % 2 != 0) return null
        val out = ByteArray(hex.length / 2)
        for (i in out.indices) {
            val hi = hexDigit(hex[i * 2]) ?: return null
            val lo = hexDigit(hex[i * 2 + 1]) ?: return null
            out[i] = ((hi shl 4) or lo).toByte()
        }
        return out
    }

    private fun hexDigit(c: Char): Int? = when (c) {
        in '0'..'9' -> c - '0'
        in 'a'..'f' -> c - 'a' + 10
        in 'A'..'F' -> c - 'A' + 10
        else -> null
    }

    /**
     * 严格 X.Y.Z 三段数值序比较;一切非法输入 → null(镜像 mtdss_core::
     * update::version_cmp:非三段/空段/非 ASCII 数字(含空白、v 前缀、+ 号、
     * pre-release 后缀)/u64 溢出全部拒绝)。
     */
    fun versionCompare(a: String, b: String): Int? {
        fun parse(v: String): LongArray? {
            val segs = v.split('.')
            if (segs.size != 3) return null
            val out = LongArray(3)
            for (i in 0 until 3) {
                val seg = segs[i]
                if (seg.isEmpty() || !seg.all { it in '0'..'9' }) return null
                out[i] = seg.toLongOrNull() ?: return null
            }
            return out
        }
        val x = parse(a) ?: return null
        val y = parse(b) ?: return null
        for (i in 0 until 3) {
            if (x[i] != y[i]) return if (x[i] < y[i]) -1 else 1
        }
        return 0
    }
}
