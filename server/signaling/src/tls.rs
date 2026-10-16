//! TLS 终止 + 开发用自签 CA/证书生成。

use anyhow::{Context, Result};
use std::path::Path;

/// 进程级安装 ring provider(rustls 0.23 需要;重复安装忽略)。
pub fn ensure_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// 以 rustls 终止 TLS 服务 axum 路由(单端口,仅 TLS)。
pub async fn serve_tls(
    app: axum::Router,
    addr: std::net::SocketAddr,
    cert: &Path,
    key: &Path,
) -> Result<()> {
    ensure_provider();
    let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
        .await
        .context("load tls cert/key")?;
    axum_server::bind_rustls(addr, config)
        .serve(app.into_make_service())
        .await
        .context("tls serve")?;
    Ok(())
}

/// 生成自签 CA + 服务端证书(开发/CI 用):ca.crt / server.crt / server.key。
/// dir 可不存在(自动创建);任一目标文件已存在即拒绝覆盖。
/// SAN 含 localhost 与回环 v4/v6 IP,客户端可用 https://127.0.0.1:port 直连。
pub fn generate_dev_tls(dir: &Path) -> Result<()> {
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, SanType};
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let (ca_p, crt_p, key_p) =
        (dir.join("ca.crt"), dir.join("server.crt"), dir.join("server.key"));
    anyhow::ensure!(
        !ca_p.exists() && !crt_p.exists() && !key_p.exists(),
        "refusing to overwrite existing tls files in {}",
        dir.display()
    );

    let now = time::OffsetDateTime::now_utc();
    let ca_key = KeyPair::generate()?;
    let mut ca = CertificateParams::new(vec![])?;
    ca.distinguished_name.push(DnType::CommonName, "MTDSS Dev CA");
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.not_before = now - time::Duration::days(1);
    ca.not_after = now + time::Duration::days(3650);
    let ca_cert = ca.self_signed(&ca_key)?;

    let server_key = KeyPair::generate()?;
    let mut leaf = CertificateParams::new(vec!["localhost".to_string()])?;
    leaf.distinguished_name.push(DnType::CommonName, "localhost");
    leaf.subject_alt_names
        .push(SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)));
    leaf.subject_alt_names
        .push(SanType::IpAddress(std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)));
    leaf.not_before = now - time::Duration::days(1);
    leaf.not_after = now + time::Duration::days(3650);
    let leaf_cert = leaf.signed_by(&server_key, &ca_cert, &ca_key)?;

    std::fs::write(&ca_p, ca_cert.pem())?;
    std::fs::write(&crt_p, leaf_cert.pem())?;
    write_private(&key_p, &server_key.serialize_pem())?;
    Ok(())
}

fn write_private(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}
