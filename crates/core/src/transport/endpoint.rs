//! QUIC 端点与双向证书固定（mutual SPKI pinning）。
//!
//! 安全语义（计划规定，M1）：
//! - 仅 TLS 1.3，仅 ED25519 签名方案；
//! - 客户端校验服务端叶子证书 SPKI == `spki_ed25519(expected peer)`；
//! - 服务端校验客户端叶子证书 SPKI 在 allowed 列表内；
//! - 双向 CertificateVerify 都做真实签名校验（证明对端持有对应私钥，
//!   防止只重放公开证书的冒充）；
//! - ALPN 双方固定 `mtdss/1`，不匹配即握手失败；
//! - TLS 1.3 下客户端本地握手（1-RTT）早于服务端校验客户端证书完成，
//!   因此 accept_pinned 会在每条连接的首条服务端双向流上写 1 字节接受
//!   令牌，connect_pinned 收到令牌才返回 Ok——被拒客户端在连接建立
//!   阶段即报错，而非等到首次用流。应用层流从第二条服务端双向流开始。
//!
//! 证书自签名不做额外校验：SPKI pin 已绑定密钥身份，配合 CertificateVerify
//! 签名校验即构成完整的密钥持有证明（同 SSH host key pin 模型）。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::crypto::{ring, CryptoProvider};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, UnixTime};
use rustls::version::TLS13;

use crate::identity::{spki_ed25519, DeviceIdentity};

/// 本协议 ALPN；两侧一致，不匹配握手失败。
pub const ALPN_QUIC: &[&[u8]] = &[b"mtdss/1"];

/// 服务端接受令牌：accept_pinned 在每条连接的首条服务端双向流上写出，
/// connect_pinned 读到它才返回 Ok（详见 connect_pinned 注释）。
const CONFIRM_TOKEN: u8 = 0x01;

/// 证书叶子 SPKI 是否等于期望的 44 字节 Ed25519 SPKI。
/// 解析失败（畸形证书）一律视为不匹配。
pub fn spki_matches(cert: &CertificateDer<'_>, expected: &[u8; 44]) -> bool {
    match x509_parser::parse_x509_certificate(cert.as_ref()) {
        Ok((_, parsed)) => parsed.public_key().raw == expected,
        Err(_) => false,
    }
}

/// 固定使用 ring provider（workspace rustls 只开 ring feature）。
fn provider() -> Arc<CryptoProvider> {
    Arc::new(ring::default_provider())
}

fn alpn_protocols() -> Vec<Vec<u8>> {
    ALPN_QUIC.iter().map(|p| p.to_vec()).collect()
}

// ---------- 客户端侧：固定服务端证书 ----------

/// 校验服务端证书：叶子 SPKI 必须精确等于期望对端的 Ed25519 SPKI；
/// TLS 1.3 CertificateVerify 用 ring 做真实签名校验。
#[derive(Debug)]
struct PinnedServerVerifier {
    expected_spki: [u8; 44],
    provider: Arc<CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for PinnedServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if spki_matches(end_entity, &self.expected_spki) {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::BadEncoding,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 is disabled".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![rustls::SignatureScheme::ED25519]
    }

    fn requires_raw_public_keys(&self) -> bool {
        false
    }
}

// ---------- 服务端侧：校验客户端证书 ----------

/// 校验客户端证书：叶子 SPKI 必须在 allowed 集合内；
/// TLS 1.3 CertificateVerify 用 ring 做真实签名校验。
#[derive(Debug)]
struct DeviceClientCertVerifier {
    /// spki -> device_id（由 allowed 公钥重算）
    allowed: HashMap<[u8; 44], String>,
    provider: Arc<CryptoProvider>,
}

impl rustls::server::danger::ClientCertVerifier for DeviceClientCertVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        if self
            .allowed
            .keys()
            .any(|spki| spki_matches(end_entity, spki))
        {
            Ok(rustls::server::danger::ClientCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::BadEncoding,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 is disabled".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![rustls::SignatureScheme::ED25519]
    }
}

// ---------- rustls 配置 ----------

fn private_key(me: &DeviceIdentity) -> Result<PrivateKeyDer<'static>> {
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    let pkcs8 = me
        .signing
        .to_pkcs8_der()
        .context("encode ed25519 signing key as pkcs8")?;
    PrivateKeyDer::try_from(pkcs8.as_bytes().to_vec())
        .map_err(|e| anyhow!("parse pkcs8 private key: {e}"))
}

fn client_rustls_config(
    me: &DeviceIdentity,
    peer: &ed25519_dalek::VerifyingKey,
) -> Result<rustls::ClientConfig> {
    let provider = provider();
    let mut cfg = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&TLS13])
        .context("TLS 1.3 unsupported by provider")?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedServerVerifier {
            expected_spki: spki_ed25519(peer),
            provider,
        }))
        .with_client_auth_cert(vec![CertificateDer::from(me.cert_der())], private_key(me)?)?;
    cfg.alpn_protocols = alpn_protocols();
    Ok(cfg)
}

fn server_rustls_config(
    me: &DeviceIdentity,
    allowed: &HashMap<String, ed25519_dalek::VerifyingKey>,
) -> Result<rustls::ServerConfig> {
    let provider = provider();
    let allowed_spki: HashMap<[u8; 44], String> = allowed
        .iter()
        .map(|(device_id, pk)| (spki_ed25519(pk), device_id.clone()))
        .collect();
    let mut cfg = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&TLS13])
        .context("TLS 1.3 unsupported by provider")?
        .with_client_cert_verifier(Arc::new(DeviceClientCertVerifier {
            allowed: allowed_spki,
            provider,
        }))
        .with_single_cert(vec![CertificateDer::from(me.cert_der())], private_key(me)?)?;
    cfg.alpn_protocols = alpn_protocols();
    Ok(cfg)
}

// ---------- 公开 API ----------

/// 客户端起点：仅绑定 UDP，不装任何服务端配置；
/// 每次连接用 `connect_pinned` 现场构造 pin 配置。
pub fn make_endpoint(bind: SocketAddr) -> Result<quinn::Endpoint> {
    let ep = quinn::Endpoint::client(bind)?;
    Ok(ep)
}

/// 服务端模式：绑定 + TLS1.3/ED25519 + 客户端证书 SPKI 白名单。
pub fn make_server_endpoint(
    bind: SocketAddr,
    me: &DeviceIdentity,
    allowed: &HashMap<String, ed25519_dalek::VerifyingKey>,
) -> Result<quinn::Endpoint> {
    let quic_server = QuicServerConfig::try_from(server_rustls_config(me, allowed)?)?;
    let server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_server));
    Ok(quinn::Endpoint::server(server_config, bind)?)
}

/// 以固定对端公钥发起连接：握手要求服务端叶子证书 SPKI == peer 的
/// Ed25519 SPKI，否则失败。
pub async fn connect_pinned(
    ep: &quinn::Endpoint,
    addr: SocketAddr,
    peer: &ed25519_dalek::VerifyingKey,
    me: &DeviceIdentity,
) -> Result<quinn::Connection> {
    let quic_client = QuicClientConfig::try_from(client_rustls_config(me, peer)?)?;
    let config = quinn::ClientConfig::new(Arc::new(quic_client));
    // server_name 不参与校验（SPKI pin 模型），传证书 CN 占位
    let conn = ep
        .connect_with(config, addr, "mtdss-device")?
        .await
        .context("pinned QUIC handshake failed")?;

    // TLS 1.3 客户端在 1-RTT 即完成本地握手，早于服务端校验客户端证书；
    // 必须等待服务端的接受确认（accept_pinned 在首条服务端双向流上写
    // 1 字节令牌），否则未列名客户端只能在首次用流时才发现被拒。
    let (mut confirm_tx, mut confirm_rx) = conn
        .accept_bi()
        .await
        .context("server did not accept this client (no confirmation stream)")?;
    // 本端不发送数据，立即半关闭发送侧
    confirm_tx.finish()?;
    let mut token = [0u8; 1];
    confirm_rx
        .read_exact(&mut token)
        .await
        .context("read server acceptance token")?;
    if token[0] != CONFIRM_TOKEN {
        return Err(anyhow!(
            "unexpected server acceptance token 0x{:02x}",
            token[0]
        ));
    }
    Ok(conn)
}

/// 接受一条已通过白名单校验的连接，返回 (连接, 对端 device_id)。
///
/// 握手期间端点内的 `DeviceClientCertVerifier` 已强制客户端证书 SPKI
/// 在 allowed 内；此处从实际呈现的证书重新提取 SPKI 反查 device_id，
/// 与 allowed 不一致（例如两处 map 不同步）即报错。
pub async fn accept_pinned(
    ep: &quinn::Endpoint,
    allowed: &HashMap<String, ed25519_dalek::VerifyingKey>,
) -> Result<(quinn::Connection, String)> {
    let incoming = ep
        .accept()
        .await
        .ok_or_else(|| anyhow!("endpoint closed while waiting for incoming connection"))?;
    let conn = incoming.await.context("pinned QUIC handshake failed")?;

    let certs = conn
        .peer_identity()
        .and_then(|i| i.downcast::<Vec<CertificateDer<'static>>>().ok())
        .ok_or_else(|| anyhow!("peer presented no client certificate"))?;
    let first = certs
        .first()
        .ok_or_else(|| anyhow!("peer presented an empty certificate chain"))?;

    let (_, parsed) = x509_parser::parse_x509_certificate(first.as_ref())
        .map_err(|e| anyhow!("parse peer certificate: {e:?}"))?;
    let raw = parsed.public_key().raw;
    if raw.len() != 44 {
        return Err(anyhow!(
            "peer cert SPKI is {} bytes, expected 44 (Ed25519)",
            raw.len()
        ));
    }
    let mut spki = [0u8; 44];
    spki.copy_from_slice(raw);

    let device_id = allowed
        .iter()
        .find(|(_, pk)| spki_ed25519(pk) == spki)
        .map(|(device_id, _)| device_id.clone())
        .ok_or_else(|| anyhow!("peer SPKI not found in allowed list"))?;

    // 发送接受令牌：客户端的 connect_pinned 据此确认服务端已接受其
    // 证书（TLS 1.3 下客户端本地握手早于服务端的客户端证书校验完成，
    // 见 connect_pinned 注释）。该连接的首条服务端双向流由此保留给
    // 确认信号，应用层流从第二条开始。
    let (mut confirm_tx, mut confirm_rx) =
        conn.open_bi().await.context("open confirmation stream")?;
    confirm_tx
        .write_all(&[CONFIRM_TOKEN])
        .await
        .context("write acceptance token")?;
    confirm_tx.finish()?;
    let _ = confirm_rx.stop(quinn::VarInt::from_u32(0));

    Ok((conn, device_id))
}
