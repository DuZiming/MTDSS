//! STUN 服务端入口:--bind/--bind-alt 双端口,默认 0.0.0.0:3478/:3479。

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();
    let mut bind = "0.0.0.0:3478".to_string();
    let mut bind_alt = "0.0.0.0:3479".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--bind" => bind = args.next().ok_or_else(|| anyhow::anyhow!("--bind needs a value"))?,
            "--bind-alt" => {
                bind_alt = args.next().ok_or_else(|| anyhow::anyhow!("--bind-alt needs a value"))?
            }
            other => anyhow::bail!("unknown flag {other}; usage: stun-server [--bind a:p] [--bind-alt a:p]"),
        }
    }
    let a = std::net::UdpSocket::bind(&bind)?;
    let b = std::net::UdpSocket::bind(&bind_alt)?;
    tracing::info!("stun on {bind} and {bind_alt}");
    let ha = tokio::spawn(stun::run_listener(a, 20, 20));
    let hb = tokio::spawn(stun::run_listener(b, 20, 20));
    let (ra, rb) = (ha.await?, hb.await?);
    ra?; rb?;
    Ok(())
}
