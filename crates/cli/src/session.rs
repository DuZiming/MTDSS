//! 会话（session.json）与对端公钥 TOFU 固定缓存（peers.json）。
//!
//! TOFU 语义（Trust On First Use，SSH known-hosts 模型）：某个 device_id
//! 的公钥第一次出现时记录；之后每一次出现都必须与记录一致，任何变更都是
//! 硬错误——身份密钥不应更换，变更意味着服务器目录被篡改或中间人攻击。

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 已登记会话：服务器地址 + 访问/刷新令牌 + 本机 device_id。
/// 存放在 `~/.mtdss/session.json`。
#[derive(Serialize, Deserialize, Clone)]
pub struct Session {
    pub server: String, // http(s)://host:port
    pub jwt: String,
    pub refresh: String,
    pub device_id: String,
    /// 服务端 TLS CA 证书(自签场景);None = 系统/webpki 根(域名证书)。
    #[serde(default)]
    pub ca: Option<PathBuf>,
}

/// MTDSS 数据目录：`$MTDSS_HOME` 覆盖，否则 `$HOME`/`%USERPROFILE%` 下的 `.mtdss`。
pub fn mtdss_home() -> PathBuf {
    if let Ok(p) = std::env::var("MTDSS_HOME") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    Path::new(&home).join(".mtdss")
}

/// 写入应保持私有的落盘文件（会话令牌、TOFU 缓存）：Unix 上 0600，
/// Windows 沿用用户目录默认 ACL。与 identity.seed 的落盘口径一致。
pub fn write_private(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

impl Session {
    pub fn load(home: &Path) -> Result<Session> {
        let p = home.join("session.json");
        let s =
            std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
        Ok(serde_json::from_str(&s)?)
    }

    pub fn save(&self, home: &Path) -> Result<()> {
        write_private(
            &home.join("session.json"),
            &serde_json::to_string_pretty(self)?,
        )
    }
}

/// 对端公钥缓存：device_id -> 公钥 hex（Ed25519 验证公钥 32 字节）。
#[derive(Serialize, Deserialize, Default)]
pub struct PeersCache {
    pub peers: HashMap<String, String>,
}

impl PeersCache {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_private(path, &serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// TOFU：首次记录该 device_id 的公钥；之后必须一致，变更即硬错误。
    /// 校验通过后由调用方 `save` 持久化（首次记录要落盘）。
    pub fn expected_peer(
        &mut self,
        device_id: &str,
        pk: &ed25519_dalek::VerifyingKey,
    ) -> Result<()> {
        let hexk = hex::encode(pk.as_bytes());
        match self.peers.get(device_id) {
            None => {
                self.peers.insert(device_id.into(), hexk);
                Ok(())
            }
            Some(prev) if *prev == hexk => Ok(()),
            Some(prev) => Err(anyhow!(
                "SECURITY: pubkey for {device_id} changed ({prev} -> {hexk}); \
                 possible MITM. Refusing."
            )),
        }
    }
}
