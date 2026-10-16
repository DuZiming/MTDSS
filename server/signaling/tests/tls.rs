//! TLS 服务器:dev 证书生成 + https 路由可达。

#[test]
fn dev_tls_generates_and_refuses_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    signaling::tls::generate_dev_tls(dir.path()).unwrap();
    assert!(dir.path().join("ca.crt").is_file());
    assert!(dir.path().join("server.crt").is_file());
    assert!(dir.path().join("server.key").is_file());
    assert!(
        signaling::tls::generate_dev_tls(dir.path()).is_err(),
        "must refuse to overwrite existing tls files"
    );
}

#[tokio::test]
async fn tls_server_serves_https_with_trusted_ca() {
    let (addr, ca, _keep) = signaling::spawn_test_tls_server().await;
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
