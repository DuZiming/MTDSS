use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();
    let db_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "signaling.db".into());
    let db = signaling::db::open(&db_path)?;
    let state = Arc::new(signaling::AppState {
        db: std::sync::Mutex::new(db),
        jwt_secret: rand::random(),
        presence: signaling::PresenceRegistry::new(),
    });
    let app = signaling::build_router(state);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8787").await?;
    tracing::info!("signaling server on 0.0.0.0:8787");
    axum::serve(listener, app).await?;
    Ok(())
}
