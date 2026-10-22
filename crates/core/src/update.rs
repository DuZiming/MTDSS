//! 签名更新清单:update manifest 类型 + canonical ed25519 验签 + 严格版本比较。
//!
//! 信任模型:签名覆盖除 `signature` 外的全部字段。canonical 字节 = 剔除
//! `signature` 后的紧凑 JSON,serde_json 默认 BTreeMap ⇒ 键恒定字典序
//! (工作区未启用 preserve_order,须保持)。
//! fail-closed 不变量:`verify` 对 hex 解码失败 / 签名长度错误 / 密钥不符
//! 一律 `false`,绝不 panic;`version_cmp` 对一切非严格 "X.Y.Z"(三段 u64,
//! 无 v 前缀、无 pre-release、无多余段、无空白)返回 `None`,调用方按无效
//! manifest 拒绝。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 单平台资产(`platforms` 的 value;键形如 "windows-x86_64")。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformAsset {
    pub url: String,
    /// 资产文件 SHA-256(hex)
    pub sha256: String,
    /// 资产字节数
    pub size: u64,
}

/// 签名更新清单。`signature` = hex(ed25519),覆盖 canonical(除 signature
/// 外全域,见 [`UpdateManifest::canonical_bytes`])。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateManifest {
    /// 目标版本,严格 "X.Y.Z"(见 [`version_cmp`])
    pub version: String,
    pub notes: String,
    pub pubdate: String,
    /// 平台 → 资产;BTreeMap 保证键序与 canonical 字节一致
    pub platforms: std::collections::BTreeMap<String, PlatformAsset>,
    /// hex(ed25519) 签名
    pub signature: String,
}

impl UpdateManifest {
    /// 剔除 `signature` 后的 JSON 值 {version,notes,pubdate,platforms}。
    /// serde_json::Map 默认 BTreeMap ⇒ 键恒定字典序(工作区禁用 preserve_order)。
    pub fn stripped_value(&self) -> serde_json::Value {
        serde_json::json!({
            "version": self.version,
            "notes": self.notes,
            "pubdate": self.pubdate,
            "platforms": self.platforms,
        })
    }

    /// canonical 字节:stripped_value 的紧凑 JSON(键排序)。签名即覆盖此字节串。
    pub fn canonical_bytes(&self) -> Vec<u8> {
        self.stripped_value().to_string().into_bytes()
    }

    /// 验签:canonical_bytes + hex 签名。fail-closed——hex 解码失败、签名
    /// 长度 != 64 字节、密钥不符一律 `false`,绝不 panic。
    pub fn verify(&self, vk: &ed25519_dalek::VerifyingKey) -> bool {
        use ed25519_dalek::{Signature, Verifier};
        let Ok(sig_bytes) = hex::decode(&self.signature) else {
            return false;
        };
        let Ok(sig) = Signature::from_slice(&sig_bytes) else {
            return false;
        };
        vk.verify(&self.canonical_bytes(), &sig).is_ok()
    }
}

/// 就地签名:对 canonical_bytes 签名,hex 写入 `signature` 字段。
pub fn sign_manifest(m: &mut UpdateManifest, sk: &ed25519_dalek::SigningKey) {
    use ed25519_dalek::Signer;
    m.signature = hex::encode(sk.sign(&m.canonical_bytes()).to_bytes());
}

/// 严格 "X.Y.Z" 三段 u64 版本比较,数值逐段比较(规避 "1.2.0" > "1.10.0"
/// 的字典序陷阱)。一切非法输入 → `None`(调用方按无效 manifest 拒绝):
/// 非三段、空段、非 ASCII 数字字符(含空白/`v` 前缀/`+` 号/pre-release
/// 后缀)、u64 溢出。
pub fn version_cmp(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    fn parse(v: &str) -> Option<[u64; 3]> {
        let mut segs = [0u64; 3];
        let mut it = v.split('.');
        for slot in &mut segs {
            let seg = it.next()?;
            // 先做纯数字白名单(比逐段 parse 更严:拒绝 "+1" 等 u64 可解析形态)
            if seg.is_empty() || !seg.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            *slot = seg.parse().ok()?;
        }
        it.next().is_none().then_some(segs)
    }
    let (a, b) = (parse(a)?, parse(b)?);
    Some(a.cmp(&b))
}

/// 下载资产复核用:字节流的 SHA-256(hex 小写)。
pub fn asset_sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use std::cmp::Ordering;

    // 测试资产哈希常量(各 64 个 hex 字符);len 断言防手写错位
    const AA64: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const BB64: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn test_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn sample_manifest() -> UpdateManifest {
        let mut platforms = std::collections::BTreeMap::new();
        platforms.insert(
            "windows-x86_64".to_string(),
            PlatformAsset {
                url: "https://dl.example.invalid/win/1.2.3/setup.exe".to_string(),
                sha256: AA64.to_string(),
                size: 123_456,
            },
        );
        platforms.insert(
            "linux-x86_64".to_string(),
            PlatformAsset {
                url: "https://dl.example.invalid/linux/1.2.3/app.tar.gz".to_string(),
                sha256: BB64.to_string(),
                size: 4_242,
            },
        );
        UpdateManifest {
            version: "1.2.3".to_string(),
            notes: "bug fixes".to_string(),
            pubdate: "2026-10-21T00:00:00Z".to_string(),
            platforms,
            signature: String::new(),
        }
    }

    fn signed() -> UpdateManifest {
        let mut m = sample_manifest();
        sign_manifest(&mut m, &test_key(1));
        m
    }

    #[test]
    fn hex_constants_are_64_chars() {
        assert_eq!(AA64.len(), 64);
        assert_eq!(BB64.len(), 64);
    }

    #[test]
    fn sign_verify_roundtrip() {
        let m = signed();
        assert!(m.verify(&test_key(1).verifying_key()));
    }

    #[test]
    fn tamper_any_signed_field_breaks_verify() {
        let cases: [fn(&mut UpdateManifest); 6] = [
            |m| m.version = "9.9.9".into(),
            |m| m.notes = "malicious".into(),
            |m| m.pubdate = "1999-01-01T00:00:00Z".into(),
            |m| {
                m.platforms.get_mut("windows-x86_64").unwrap().url =
                    "https://evil.invalid/setup.exe".into()
            },
            |m| m.platforms.get_mut("windows-x86_64").unwrap().sha256 = AA64.replace('a', "c"),
            |m| m.platforms.get_mut("linux-x86_64").unwrap().size += 1,
        ];
        for (i, tamper) in cases.into_iter().enumerate() {
            let mut m = signed();
            tamper(&mut m);
            assert!(
                !m.verify(&test_key(1).verifying_key()),
                "tamper case {i} unexpectedly verified"
            );
        }
    }

    #[test]
    fn signature_tamper_and_encoding_failures() {
        let vk = test_key(1).verifying_key();
        let wrong_vk = test_key(2).verifying_key();

        // 非 hex
        let mut m = signed();
        m.signature = "zz".repeat(64);
        assert!(!m.verify(&vk));

        // 空
        let mut m = signed();
        m.signature = String::new();
        assert!(!m.verify(&vk));

        // 长度错误:32 字节
        let mut m = signed();
        m.signature = hex::encode([0u8; 32]);
        assert!(!m.verify(&vk));

        // 长度错误:65 字节
        let mut m = signed();
        m.signature = hex::encode([0u8; 65]);
        assert!(!m.verify(&vk));

        // 奇数长度 hex(63 字符)
        let mut m = signed();
        m.signature = "a".repeat(63);
        assert!(!m.verify(&vk));

        // 合法编码(64B 全零)但签名不匹配
        let mut m = signed();
        m.signature = hex::encode([0u8; 64]);
        assert!(!m.verify(&vk));

        // 错误密钥
        let m = signed();
        assert!(!m.verify(&wrong_vk), "wrong key must not verify");
    }

    #[test]
    fn canonical_bytes_exclude_signature() {
        let signed_m = signed();
        let unsigned_m = sample_manifest(); // signature 为空
        assert_eq!(
            signed_m.canonical_bytes(),
            unsigned_m.canonical_bytes(),
            "canonical must not depend on the signature field"
        );
        let mut alt_sig = sample_manifest();
        alt_sig.signature = "deadbeef".into();
        assert_eq!(signed_m.canonical_bytes(), alt_sig.canonical_bytes());
    }

    #[test]
    fn canonical_bytes_deterministic_and_sorted() {
        let m = signed();
        assert_eq!(m.canonical_bytes(), m.canonical_bytes());
        // 白板手写 JSON:顶层 notes < platforms < pubdate < version;
        // 平台名 linux-x86_64 < windows-x86_64;asset 内 sha256 < size < url。
        let expected = concat!(
            r#"{"notes":"bug fixes","platforms":{"#,
            r#""linux-x86_64":{"sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","size":4242,"url":"https://dl.example.invalid/linux/1.2.3/app.tar.gz"},"#,
            r#""windows-x86_64":{"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size":123456,"url":"https://dl.example.invalid/win/1.2.3/setup.exe"}},"#,
            r#""pubdate":"2026-10-21T00:00:00Z","version":"1.2.3"}"#,
        );
        assert_eq!(
            String::from_utf8(m.canonical_bytes()).unwrap(),
            expected,
            "canonical bytes must be compact JSON with sorted keys"
        );
    }

    #[test]
    fn serde_roundtrip_preserves_verifiability() {
        let m = signed();
        let json = serde_json::to_string(&m).unwrap();
        let back: UpdateManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
        assert!(back.verify(&test_key(1).verifying_key()));
    }

    #[test]
    fn version_cmp_numerical_not_lexicographic() {
        assert_eq!(version_cmp("1.0.0", "1.0.1"), Some(Ordering::Less));
        // 字典序陷阱:字符串比较 "1.2.0" > "1.10.0",数值比较必须 2 < 10
        assert_eq!(version_cmp("1.2.0", "1.10.0"), Some(Ordering::Less));
        assert_eq!(version_cmp("1.10.0", "1.2.0"), Some(Ordering::Greater));
        assert_eq!(version_cmp("2.0.0", "1.99.99"), Some(Ordering::Greater));
        assert_eq!(version_cmp("1.2.3", "1.2.3"), Some(Ordering::Equal));
        assert_eq!(version_cmp("0.0.0", "0.0.0"), Some(Ordering::Equal));
    }

    #[test]
    fn version_cmp_rejects_non_strict_xyz() {
        for bad in [
            "1.0",                      // 两段
            "1.0.0.0",                  // 四段
            "1.0.0-rc1",                // pre-release
            "v1.0.0",                   // v 前缀
            "1..0",                     // 空段
            ".1.0",                     // 空主段
            "1.0.",                     // 尾空段
            "",                         // 空
            "1.0.x",                    // 非数字段
            " 1.0.0",                   // 前导空白
            "1.0.0 ",                   // 尾随空白
            "1.0.0\n",                  // 尾随换行
            "+1.0.0",                   // '+' 前缀(u64 parse 单独会接受)
            "99999999999999999999.0.0", // u64 溢出
        ] {
            assert_eq!(version_cmp(bad, "1.0.0"), None, "a={bad:?} must be None");
            assert_eq!(version_cmp("1.0.0", bad), None, "b={bad:?} must be None");
        }
    }

    #[test]
    fn asset_sha256_known_vectors() {
        assert_eq!(
            asset_sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            asset_sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
