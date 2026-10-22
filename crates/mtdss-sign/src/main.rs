//! mtdss-sign —— 更新清单签名助手(工作区小工具,双平台可跑)。
//!
//! 四个子命令:
//! - `keygen`:生成 ed25519 密钥对,hex 落盘 update-pub.hex / update-priv.hex
//!   (已存在则拒,绝不静默覆盖);两枚 key 均随 stdout 打印。
//! - `manifest`:由本地资产文件生成未签名清单(signature 为空串;sha256+size
//!   实算,asset url = `{url-base}/win/<ver>/<文件名>`,pubdate = now UTC)。
//!   version 非严格三段 X.Y.Z 直接拒(`version_cmp` 为 None 不收)。
//! - `sign`:读私钥 hex,对 canonical 域签名,写已签 JSON。
//! - `verify`:读公钥 hex 验签;退出码 0 = 有效,1 = 无效/错误。
//!
//! canonical/验签逻辑全部复用 `mtdss_core::update`(单一事实源),本工具不
//! 实现第二套。私钥文件落盘沿仓库 `write_private` 口径:Unix 0600,Windows
//! 沿用用户目录默认 ACL。退出码:0 成功;1 失败(业务错误/验签不过);
//! clap 参数错误自带 2。

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use mtdss_core::update::{asset_sha256_hex, sign_manifest, version_cmp, PlatformAsset, UpdateManifest};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "mtdss-sign", version, about = "MTDSS 更新清单签名助手(ed25519)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 生成 ed25519 密钥对:update-pub.hex + update-priv.hex(已存在则拒)
    Keygen {
        /// 输出目录(缺省当前目录)
        #[arg(long, default_value = ".")]
        out: PathBuf,
    },
    /// 生成未签名清单(signature 为空串;sha256/size 实算自资产文件)
    Manifest {
        /// 目标版本(严格 X.Y.Z 三段数字)
        #[arg(long)]
        version: String,
        /// 升级说明文本文件(全文作为 notes)
        #[arg(long)]
        notes_file: PathBuf,
        /// 下载基地址(如 https://host/dl)
        #[arg(long)]
        url_base: String,
        /// 资产文件(本地路径;url 取其文件名)
        #[arg(long)]
        file: PathBuf,
        /// 平台键
        #[arg(long, default_value = "windows-x86_64")]
        platform: String,
        /// 输出 JSON 路径
        #[arg(long)]
        out: PathBuf,
    },
    /// 对清单签名(读私钥 hex,写已签 JSON)
    Sign {
        /// 未签名清单 JSON
        #[arg(long)]
        manifest: PathBuf,
        /// 私钥 hex 文件(update-priv.hex)
        #[arg(long)]
        key: PathBuf,
        /// 输出已签清单 JSON
        #[arg(long)]
        out: PathBuf,
    },
    /// 验签(退出码 0 = 有效,1 = 无效/错误)
    Verify {
        /// 已签清单 JSON
        #[arg(long)]
        manifest: PathBuf,
        /// 公钥 hex 文件(update-pub.hex)
        #[arg(long)]
        pubkey: PathBuf,
    },
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match dispatch(cli.cmd) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn dispatch(cmd: Cmd) -> Result<std::process::ExitCode> {
    match cmd {
        Cmd::Keygen { out } => keygen(&out).map(|_| std::process::ExitCode::SUCCESS),
        Cmd::Manifest {
            version,
            notes_file,
            url_base,
            file,
            platform,
            out,
        } => manifest(&version, &notes_file, &url_base, &file, &platform, &out)
            .map(|_| std::process::ExitCode::SUCCESS),
        Cmd::Sign {
            manifest,
            key,
            out,
        } => sign(&manifest, &key, &out).map(|_| std::process::ExitCode::SUCCESS),
        Cmd::Verify { manifest, pubkey } => verify(&manifest, &pubkey),
    }
}

/// 敏感文件(私钥)落盘:Unix 0600;Windows 沿用用户目录默认 ACL。
/// 与 daemon session.rs 的 write_private 同口径(独立 crate 的本地简单实现)。
fn write_private(path: &Path, contents: &str) -> Result<()> {
    create_parent(path)?;
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// 普通文件落盘,父目录自动创建(裸文件名的 parent 为空串,跳过)。
fn write_file(path: &Path, contents: &str) -> Result<()> {
    create_parent(path)?;
    std::fs::write(path, contents)?;
    Ok(())
}

fn create_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
    }
    Ok(())
}

fn keygen(out_dir: &Path) -> Result<()> {
    let mut seed = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut seed);
    let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
    let pub_hex = hex::encode(sk.verifying_key().to_bytes());
    let priv_hex = hex::encode(sk.to_bytes());

    let pub_path = out_dir.join("update-pub.hex");
    let priv_path = out_dir.join("update-priv.hex");
    for p in [&pub_path, &priv_path] {
        if p.exists() {
            bail!("refusing to overwrite existing key file: {}", p.display());
        }
    }
    std::fs::create_dir_all(out_dir).with_context(|| format!("create dir {}", out_dir.display()))?;
    write_private(&priv_path, &format!("{priv_hex}\n"))?;
    write_file(&pub_path, &format!("{pub_hex}\n"))?;
    println!("public key:  {pub_hex}");
    println!("private key: {priv_hex}");
    println!("written: {} + {}", pub_path.display(), priv_path.display());
    Ok(())
}

fn manifest(
    version: &str,
    notes_file: &Path,
    url_base: &str,
    file: &Path,
    platform: &str,
    out: &Path,
) -> Result<()> {
    // version 内嵌校验:version_cmp 对一切非严格三段形态返回 None,不收
    if version_cmp(version, "0.0.0").is_none() {
        bail!("invalid --version {version:?}: must be strict X.Y.Z (three numeric segments)");
    }
    let notes = std::fs::read_to_string(notes_file)
        .with_context(|| format!("read notes {}", notes_file.display()))?;
    let asset_bytes =
        std::fs::read(file).with_context(|| format!("read asset {}", file.display()))?;
    let sha256 = asset_sha256_hex(&asset_bytes);
    let size = asset_bytes.len() as u64;
    let name = file
        .file_name()
        .with_context(|| format!("--file {} has no file name", file.display()))?
        .to_string_lossy();
    let url = format!("{}/win/{version}/{name}", url_base.trim_end_matches('/'));

    let mut platforms = BTreeMap::new();
    platforms.insert(
        platform.to_string(),
        PlatformAsset {
            url,
            sha256,
            size,
        },
    );
    let m = UpdateManifest {
        version: version.to_string(),
        notes,
        pubdate: now_rfc3339_utc(),
        platforms,
        signature: String::new(),
    };
    write_file(out, &format!("{}\n", serde_json::to_string_pretty(&m)?))?;
    println!("unsigned manifest written to {}", out.display());
    Ok(())
}

/// now UTC,秒级 RFC3339(如 `2026-10-21T12:34:56Z`,与 spec 示例同形)。
fn now_rfc3339_utc() -> String {
    let fmt =
        time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
    time::OffsetDateTime::now_utc()
        .format(fmt)
        .expect("RFC3339 format of now_utc cannot fail")
}

fn sign(manifest_path: &Path, key: &Path, out: &Path) -> Result<()> {
    let mut m = read_manifest(manifest_path)?;
    let sk = read_signing_key(key)?;
    sign_manifest(&mut m, &sk);
    write_file(out, &format!("{}\n", serde_json::to_string_pretty(&m)?))?;
    println!("signed manifest written to {}", out.display());
    Ok(())
}

fn verify(manifest_path: &Path, pubkey: &Path) -> Result<std::process::ExitCode> {
    let m = read_manifest(manifest_path)?;
    let vk = read_verifying_key(pubkey)?;
    if m.verify(&vk) {
        println!("OK: signature valid for version {}", m.version);
        Ok(std::process::ExitCode::SUCCESS)
    } else {
        eprintln!(
            "INVALID: signature verification failed for {}",
            manifest_path.display()
        );
        Ok(std::process::ExitCode::FAILURE)
    }
}

fn read_manifest(path: &Path) -> Result<UpdateManifest> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse manifest {}", path.display()))
}

/// 密钥 hex 文件 → 字节:去首尾空白后须为 64 个 hex 字符(32 字节)。
fn decode_key_hex(text: &str, what: &str, path: &Path) -> Result<[u8; 32]> {
    let h = text.trim();
    if h.len() != 64 {
        bail!("{what} in {} must be 64 hex chars (32 bytes), got {}", path.display(), h.len());
    }
    let bytes = hex::decode(h).with_context(|| format!("bad {what} hex in {}", path.display()))?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("{what} in {} must be 32 bytes", path.display()))
}

fn read_signing_key(path: &Path) -> Result<ed25519_dalek::SigningKey> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let seed = decode_key_hex(&text, "private key", path)?;
    Ok(ed25519_dalek::SigningKey::from_bytes(&seed))
}

fn read_verifying_key(path: &Path) -> Result<ed25519_dalek::VerifyingKey> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let bytes = decode_key_hex(&text, "public key", path)?;
    ed25519_dalek::VerifyingKey::from_bytes(&bytes).map_err(|e| anyhow!("invalid public key: {e}"))
}
