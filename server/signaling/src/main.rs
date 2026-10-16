use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let mut db_path = "signaling.db".to_string();
    let mut tls_cert: Option<String> = None;
    let mut tls_key: Option<String> = None;
    let mut dev_gen_tls: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--tls-cert" => {
                tls_cert = Some(args.next().ok_or_else(|| anyhow::anyhow!("--tls-cert needs a value"))?)
            }
            "--tls-key" => {
                tls_key = Some(args.next().ok_or_else(|| anyhow::anyhow!("--tls-key needs a value"))?)
            }
            "--dev-gen-tls" => dev_gen_tls = Some(
                args.next()
                    .ok_or_else(|| anyhow::anyhow!("--dev-gen-tls needs a value"))?,
            ),
            other if !other.starts_with("--") => db_path = other.to_string(),
            other => anyhow::bail!(
                "unknown flag {other}; usage: signaling-server [db] --tls-cert <pem> \
                 --tls-key <pem> | --dev-gen-tls <dir>"
            ),
        }
    }

    if let Some(dir) = dev_gen_tls {
        signaling::tls::generate_dev_tls(std::path::Path::new(&dir))?;
        println!("wrote ca.crt / server.crt / server.key in {dir}");
        println!("server: --tls-cert {dir}/server.crt --tls-key {dir}/server.key");
        println!("client: --ca {dir}/ca.crt");
        return Ok(());
    }

    let (cert, key) = match (tls_cert, tls_key) {
        (Some(c), Some(k)) => (c, k),
        _ => anyhow::bail!(
            "TLS required: pass --tls-cert <pem> --tls-key <pem> (dev: --dev-gen-tls <dir> first)"
        ),
    };

    let db = signaling::db::open(&db_path)?;
    let state = Arc::new(signaling::AppState {
        db: std::sync::Mutex::new(db),
        jwt_secret: rand::random(),
        presence: signaling::PresenceRegistry::new(),
        keepalive: signaling::ws::Keepalive::default(),
    });
    let app = signaling::build_router(state);
    let addr: std::net::SocketAddr = "0.0.0.0:8787".parse().expect("valid socket addr");
    tracing::info!("signaling server (TLS) on {addr}");
    signaling::tls::serve_tls(app, addr, std::path::Path::new(&cert), std::path::Path::new(&key))
        .await
}
