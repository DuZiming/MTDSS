//! TLS 服务器:dev 证书生成 + https 路由可达。

#[test]
fn dev_tls_generates_and_refuses_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    signaling::tls::generate_dev_tls(dir.path(), &[]).unwrap();
    assert!(dir.path().join("ca.crt").is_file());
    assert!(dir.path().join("server.crt").is_file());
    assert!(dir.path().join("server.key").is_file());
    assert!(
        signaling::tls::generate_dev_tls(dir.path(), &[]).is_err(),
        "must refuse to overwrite existing tls files"
    );
}

#[tokio::test]
async fn tls_server_serves_https_with_trusted_ca() {
    let (addr, ca, keep) = signaling::spawn_test_tls_server().await;
    // 认证收口后仅预置账号可领码:AppState 已随服务器任务移走,借返回的
    // TempDir 定位 DB 另开连接预置(db::open 幂等,WAL 下双连接安全)
    let conn = signaling::db::open(keep.path().join("t.db").to_str().unwrap()).unwrap();
    signaling::admin::user_add(&conn, "tls@x.y").unwrap();
    let pem = std::fs::read(&ca).unwrap();
    let client = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(&pem).unwrap())
        .build()
        .unwrap();
    // 公开端点 200 即证明 TLS 握手 + 路由全通
    let resp = client
        .post(format!("https://{addr}/auth/request-code"))
        .json(&serde_json::json!({ "email": "tls@x.y" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

/// 解析 server.crt 的 SAN 列表为 ("DNS"|"IP", 值) 形式,便于断言注入项。
fn cert_sans(crt: &std::path::Path) -> Vec<(String, String)> {
    use x509_parser::prelude::*;
    let data = std::fs::read(crt).unwrap();
    let (_rem, pem) = parse_x509_pem(&data).unwrap();
    let cert = pem.parse_x509().unwrap();
    let san = cert
        .subject_alternative_name()
        .unwrap()
        .expect("leaf must carry a SAN extension");
    san.value
        .general_names
        .iter()
        .map(|gn| match gn {
            GeneralName::DNSName(d) => ("DNS".to_string(), (*d).to_string()),
            GeneralName::IPAddress(ip) => {
                if ip.len() == 4 {
                    (
                        "IP".to_string(),
                        format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]),
                    )
                } else {
                    let mut octets = [0u8; 16];
                    octets.copy_from_slice(ip);
                    ("IP".to_string(), std::net::Ipv6Addr::from(octets).to_string())
                }
            }
            other => ("OTHER".to_string(), format!("{other:?}")),
        })
        .collect()
}

/// SAN 注入:IP 形态入 IpAddress、域名形态入 DnsName,原有三项保留。
#[test]
fn tls_san_injection_appends_ip_and_dns_without_dropping_defaults() {
    let dir = tempfile::tempdir().unwrap();
    signaling::tls::generate_dev_tls(
        dir.path(),
        &[
            "203.0.113.1".to_string(),
            "mtdss.example.internal".to_string(),
        ],
    )
    .unwrap();
    let sans = cert_sans(&dir.path().join("server.crt"));
    assert!(sans.contains(&("DNS".into(), "localhost".into())), "{sans:?}");
    assert!(sans.contains(&("IP".into(), "127.0.0.1".into())), "{sans:?}");
    assert!(sans.contains(&("IP".into(), "::1".into())), "{sans:?}");
    assert!(sans.contains(&("IP".into(), "203.0.113.1".into())), "{sans:?}");
    assert!(
        sans.contains(&("DNS".into(), "mtdss.example.internal".into())),
        "{sans:?}"
    );
}

/// 空注入(内部调用方 spawn_test_tls_server 形态)恰好保持原三项。
#[test]
fn tls_san_empty_slice_keeps_exactly_default_three() {
    let dir = tempfile::tempdir().unwrap();
    signaling::tls::generate_dev_tls(dir.path(), &[]).unwrap();
    assert_eq!(cert_sans(&dir.path().join("server.crt")).len(), 3);
}

/// 非 IP 且非 IA5(含非 ASCII)→ rcgen 拒绝生成,报错而非 panic/坏证书。
#[test]
fn tls_san_rejects_non_ia5_name() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        signaling::tls::generate_dev_tls(dir.path(), &["例.mtdss".to_string()]).is_err(),
        "non-IA5 SAN must fail"
    );
}
