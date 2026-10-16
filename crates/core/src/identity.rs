use anyhow::{Context, Result};
use ed25519_dalek::{Signer, Signature, SigningKey, VerifyingKey};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::path::Path;

/// Ed25519 SubjectPublicKeyInfo DER：12 字节头 + 32 字节密钥
pub const ED25519_SPKI_PREFIX: [u8; 12] =
    [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];

pub fn spki_ed25519(pk: &VerifyingKey) -> [u8; 44] {
    let mut out = [0u8; 44];
    out[..12].copy_from_slice(&ED25519_SPKI_PREFIX);
    out[12..].copy_from_slice(pk.as_bytes());
    out
}

pub fn device_id_for(pk: &VerifyingKey) -> String {
    let mut h = Sha256::new();
    h.update(pk.as_bytes());
    hex::encode(h.finalize())[..32].to_string()
}

pub struct DeviceIdentity {
    pub device_id: String,
    pub signing: SigningKey,
    cert_cache: std::sync::OnceLock<Vec<u8>>,
}

impl DeviceIdentity {
    pub fn generate() -> Self {
        let mut seed = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut seed);
        Self::from_seed(&seed)
    }

    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let signing = SigningKey::from_bytes(seed);
        let device_id = device_id_for(&signing.verifying_key());
        Self {
            device_id,
            signing,
            cert_cache: std::sync::OnceLock::new(),
        }
    }

    pub fn from_seed_file(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read seed {}", path.display()))?;
        let hexs = text.trim();
        let bytes = hex::decode(hexs).context("seed file is not hex")?;
        let seed: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("seed must be 32 bytes"))?;
        Ok(Self::from_seed(&seed))
    }

    pub fn save_seed(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, hex::encode(self.signing.to_bytes()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn public_key(&self) -> VerifyingKey {
        self.signing.verifying_key()
    }

    pub fn sign(&self, msg: &[u8]) -> Signature {
        self.signing.sign(msg)
    }

    /// Ed25519 自签证书（der），同一身份稳定
    pub fn cert_der(&self) -> Vec<u8> {
        self.cert_cache.get_or_init(|| self.build_cert()).clone()
    }

    fn build_cert(&self) -> Vec<u8> {
        use ed25519_dalek::pkcs8::EncodePrivateKey;
        use rcgen::{CertificateParams, KeyPair, PKCS_ED25519};

        let pkcs8 = self.signing.to_pkcs8_der().expect("ed25519 pkcs8 der");
        let kp = KeyPair::from_pkcs8_der_and_sign_algo(&pkcs8.as_bytes().into(), &PKCS_ED25519)
            .expect("ed25519 keypair -> rcgen");
        let mut params =
            CertificateParams::new(vec!["mtdss-device".to_string()]).expect("cert params");
        params.serial_number = Some(rcgen::SerialNumber::from(42));
        // 固定有效期窗口起始于 epoch，保证 DER 稳定
        params.not_before = time::OffsetDateTime::UNIX_EPOCH;
        params.not_after = time::OffsetDateTime::from_unix_timestamp(253402300799).unwrap();
        let cert = params.self_signed(&kp).expect("self-signed cert");
        cert.der().to_vec()
    }
}
