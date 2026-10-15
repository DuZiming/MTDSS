pub mod api;
pub mod auth;
pub mod db;
pub mod ws;

use dashmap::DashMap;
use std::net::SocketAddr;
use std::sync::Arc;

pub struct AppState {
    pub db: std::sync::Mutex<rusqlite::Connection>,
    pub jwt_secret: [u8; 32],
    pub presence: PresenceRegistry,
}

/// device_id -> 在线连接发送端
pub struct PresenceRegistry {
    // Task 6 填充更多行为；is_online 已在使用注册表
    inner: DashMap<String, crate::ws::PeerHandle>,
}

impl PresenceRegistry {
    pub fn new() -> Self {
        Self {
            inner: DashMap::new(),
        }
    }

    pub fn is_online(&self, device_id: &str) -> bool {
        self.inner.contains_key(device_id)
    }
}

impl Default for PresenceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub fn build_router(state: Arc<AppState>) -> axum::Router {
    axum::Router::new()
        .merge(api::routes())
        // Task 6: .route("/ws", axum::routing::get(ws::ws_handler))
        .with_state(state)
}

/// 集成测试基建：随机端口起服务器
pub async fn spawn_test_server() -> (SocketAddr, Arc<AppState>) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("t.db");
    let db = db::open(db_path.to_str().unwrap()).unwrap();
    let state = Arc::new(AppState {
        db: std::sync::Mutex::new(db),
        jwt_secret: rand::random(),
        presence: PresenceRegistry::new(),
    });
    let app = build_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, state)
}

/// oneshot 测试用：不起端口
pub async fn test_state() -> Arc<AppState> {
    let db = db::open(":memory:").unwrap();
    Arc::new(AppState {
        db: std::sync::Mutex::new(db),
        jwt_secret: rand::random(),
        presence: PresenceRegistry::new(),
    })
}
