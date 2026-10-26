package com.mtdss.android.update

import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.toCanonicalJson
import org.bouncycastle.crypto.params.Ed25519PrivateKeyParameters
import org.bouncycastle.crypto.signers.Ed25519Signer
import java.security.MessageDigest

/**
 * 更新面测试 fixture(UpdateCheckTest / UpdateControllerTest / SettingsViewModelTest
 * 共用):BC Ed25519 seed 密钥(与 Rust 侧测试 SigningKey::from_bytes(&[seed; 32])
 * 同构)、清单签名/JSON 构造、SHA-256/hex 助手。签名用 BC 轻量 API——与
 * App 内验签同一实现(生产验签见 UpdateCheck 类注释的选型说明)。
 */
object UpdateFixtures {

    /** 固定 seed → 确定性密钥对(镜像 Rust 测试 test_key(seed))。 */
    fun seedPrivateKey(seed: Byte): Ed25519PrivateKeyParameters {
        val buf = ByteArray(32) { seed }
        return Ed25519PrivateKeyParameters(buf, 0)
    }

    fun pubHex(seed: Byte): String = hex(seedPrivateKey(seed).generatePublicKey().getEncoded())

    fun hex(bytes: ByteArray): String =
        bytes.joinToString("") { String.format(java.util.Locale.ROOT, "%02x", it) }

    fun sha256Hex(bytes: ByteArray): String = hex(MessageDigest.getInstance("SHA-256").digest(bytes))

    /** 对 canonical 域签名并写回 signature(镜像 mtdss_core::update::sign_manifest)。 */
    fun signed(m: UpdateManifest, seed: Byte): UpdateManifest =
        m.copy(signature = signHex(seedPrivateKey(seed), m.canonicalBytes()))

    fun signHex(priv: Ed25519PrivateKeyParameters, bytes: ByteArray): String {
        val signer = Ed25519Signer()
        signer.init(true, priv)
        signer.update(bytes, 0, bytes.size)
        return hex(signer.generateSignature())
    }

    /** 全字段(含 signature)紧凑 JSON——喂回 parseManifest 的输入形态。 */
    fun manifestJson(m: UpdateManifest): String = JVal.Obj(
        "version" to JVal.str(m.version),
        "notes" to JVal.str(m.notes),
        "pubdate" to JVal.str(m.pubdate),
        "platforms" to JVal.Obj(
            LinkedHashMap<String, JVal>().apply {
                m.platforms.forEach { (k, a) ->
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
        "signature" to JVal.str(m.signature),
    ).toCanonicalJson()

    /** 单安卓资产清单(未签名;signed()/manifestJson() 配套)。 */
    fun androidManifest(version: String, asset: PlatformAsset): UpdateManifest = UpdateManifest(
        version = version,
        notes = "测试更新说明",
        pubdate = "2026-10-26T00:00:00Z",
        platforms = linkedMapOf(ANDROID_PLATFORM_KEY to asset),
        signature = "",
    )

    /** 256 字节确定性假 APK(下载复核面的载荷)。 */
    val apkBytes: ByteArray = ByteArray(256) { ((it * 31 + it / 3) and 0xFF).toByte() }

    /** 假 APK 的下载资产(sha/size 实算)。 */
    fun apkAsset(url: String): PlatformAsset = PlatformAsset(url, sha256Hex(apkBytes), apkBytes.size.toLong())
}
