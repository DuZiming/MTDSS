use std::sync::Arc;
use tracing_subscriber::EnvFilter;

const USAGE: &str = "usage: signaling-server [db] --plain-http <ip:port> \
                     | --tls-cert <pem> --tls-key <pem> | --dev-gen-tls <dir> [--tls-san <name>]... \
                     | --stun-addr <ip-or-host> --stun-port <p>[,<alt>]";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let mut db_path = "signaling.db".to_string();
    let mut plain_http: Option<String> = None;
    let mut tls_cert: Option<String> = None;
    let mut tls_key: Option<String> = None;
    let mut dev_gen_tls: Option<String> = None;
    let mut tls_sans: Vec<String> = Vec::new();
    let mut stun_addr: Option<String> = None;
    let mut stun_port: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--plain-http" => {
                plain_http = Some(args.next().ok_or_else(|| anyhow::anyhow!("--plain-http needs a value"))?)
            }
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
            // 可重复:为 --dev-gen-tls 生成的证书追加 SAN(IP 或域名)
            "--tls-san" => tls_sans.push(
                args.next()
                    .ok_or_else(|| anyhow::anyhow!("--tls-san needs a value"))?,
            ),
            "--stun-addr" => {
                stun_addr = Some(args.next().ok_or_else(|| anyhow::anyhow!("--stun-addr needs a value"))?)
            }
            "--stun-port" => {
                stun_port = Some(args.next().ok_or_else(|| anyhow::anyhow!("--stun-port needs a value"))?)
            }
            other if !other.starts_with("--") => db_path = other.to_string(),
            other => anyhow::bail!("unknown flag {other}; {USAGE}"),
        }
    }

    // 三选一:--plain-http(Caddy 等反代终止 TLS 后回源回环)或 --tls-cert/--tls-key
    // (直连/过渡期/dev CA)。同时给出即拒绝——监听形态二义;先于任何 DB/文件副作用。
    if plain_http.is_some() && (tls_cert.is_some() || tls_key.is_some()) {
        anyhow::bail!("--plain-http and --tls-cert/--tls-key are mutually exclusive; {USAGE}");
    }

    // --tls-san 只为 --dev-gen-tls 生成的证书注入 SAN,单独给出即拒绝(先于生成副作用)。
    if !tls_sans.is_empty() && dev_gen_tls.is_none() {
        anyhow::bail!("--tls-san requires --dev-gen-tls <dir>; {USAGE}");
    }

    // STUN 通告地址:HelloOk 随握手下发给客户端,做 NAT 探测候选。
    // 端口校验(1-2 个、非 0、不重复)入库 signaling::parse_stun_ports,可单测。
    let stun_host = stun_addr.unwrap_or_else(|| "127.0.0.1".to_string());
    let ports_raw = stun_port.unwrap_or_else(|| "3478,3479".to_string());
    let ports = signaling::parse_stun_ports(&ports_raw)?;
    let stun_addrs: Vec<String> = ports.iter().map(|p| format!("{stun_host}:{p}")).collect();

    if let Some(dir) = dev_gen_tls {
        signaling::tls::generate_dev_tls(std::path::Path::new(&dir), &tls_sans)?;
        println!("wrote ca.crt / server.crt / server.key in {dir}");
        if !tls_sans.is_empty() {
            println!("extra sans: {}", tls_sans.join(" "));
        }
        println!("server: --tls-cert {dir}/server.crt --tls-key {dir}/server.key");
        println!("client: --ca {dir}/ca.crt");
        return Ok(());
    }

    let plain_addr: Option<std::net::SocketAddr> = match plain_http.as_deref() {
        Some(p) => Some(p.parse().map_err(|_| {
            anyhow::anyhow!("--plain-http {p:?}: expected <ip:port>; {USAGE}")
        })?),
        None => None,
    };

    let db = signaling::db::open(&db_path)?;
    tracing::info!("signaling server stun_addrs={stun_addrs:?}");
    let state = Arc::new(signaling::AppState {
        db: std::sync::Mutex::new(db),
        jwt_secret: rand::random(),
        presence: Arc::new(signaling::PresenceRegistry::new()),
        keepalive: signaling::ws::Keepalive::default(),
        stun_addrs,
        relay: std::sync::OnceLock::new(),
    });
    let app = signaling::build_router(state);
    match plain_addr {
        Some(addr) => {
            tracing::info!("signaling server (plain http) on {addr}");
            signaling::serve_plain(app, addr).await
        }
        None => {
            let (cert, key) = match (tls_cert, tls_key) {
                (Some(c), Some(k)) => (c, k),
                _ => anyhow::bail!(
                    "TLS required: pass --tls-cert <pem> --tls-key <pem> (dev: --dev-gen-tls <dir> first)"
                ),
            };
            let addr: std::net::SocketAddr = "0.0.0.0:8787".parse().expect("valid socket addr");
            tracing::info!("signaling server (TLS) on {addr}");
            signaling::tls::serve_tls(app, addr, std::path::Path::new(&cert), std::path::Path::new(&key))
                .await
        }
    }
}
