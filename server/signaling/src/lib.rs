pub mod api;
pub mod auth;
pub mod db;
pub mod tls;
pub mod ws;

use dashmap::DashMap;
use prost::Message as _;
use std::net::SocketAddr;
use std::sync::Arc;

pub struct AppState {
    pub db: std::sync::Mutex<rusqlite::Connection>,
    pub jwt_secret: [u8; 32],
    pub presence: PresenceRegistry,
    pub keepalive: crate::ws::Keepalive,
}

impl AppState {
    /// 把 payload 转发给同账号在线设备 `to`。
    /// 返回 Err((code, message)) 表示调用方应回 Error 帧（code 2 = 目标不可达/跨账号）。
    pub fn relay(
        &self,
        from: &str,
        from_account: &str,
        to: &str,
        payload: &[u8],
    ) -> Result<(), (u32, String)> {
        let peer = self
            .presence
            .get(to)
            .ok_or((2u32, "target offline".to_string()))?;
        if peer.account_id != from_account {
            return Err((2, "cross-account relay denied".into()));
        }
        let msg = axum::extract::ws::Message::Binary(
            mtdss_proto::ws::WsServerFrame {
                frame: Some(mtdss_proto::ws::ws_server_frame::Frame::Relayed(
                    mtdss_proto::ws::Relayed {
                        from_device: from.into(),
                        payload: payload.to_vec(),
                    },
                )),
            }
            .encode_to_vec()
            .into(),
        );
        peer.tx
            .try_send(msg)
            .map_err(|_| (2, "target queue full".into()))
    }
}

/// device_id -> 在线连接发送端
pub struct PresenceRegistry {
    inner: DashMap<String, crate::ws::PeerHandle>,
}

impl PresenceRegistry {
    pub fn new() -> Self {
        Self {
            inner: DashMap::new(),
        }
    }

    pub fn insert(&self, device_id: &str, peer: crate::ws::PeerHandle) {
        self.inner.insert(device_id.to_string(), peer);
    }

    pub fn remove(&self, device_id: &str) {
        self.inner.remove(device_id);
    }

    /// 仅当该 device_id 的当前注册仍属于 tx 所在通道（即仍是本次连接自己的注册）时才移除。
    /// 断线清理专用：同设备重连覆盖注册后，旧连接退出时不得清掉新连接的注册。
    /// 返回是否真的移除了自己的注册（false = 注册已被更新的连接接管）。
    pub fn remove_if_current(
        &self,
        device_id: &str,
        tx: &tokio::sync::mpsc::Sender<axum::extract::ws::Message>,
    ) -> bool {
        self.inner
            .remove_if(&device_id.to_string(), |_, v| v.tx.same_channel(tx))
            .is_some()
    }

    pub fn is_online(&self, device_id: &str) -> bool {
        self.inner.contains_key(device_id)
    }

    /// 同账号全部在线 peer（(device_id, handle) 克隆快照；调用方自行排除发起设备）
    pub fn account_peers(&self, account_id: &str) -> Vec<(String, crate::ws::PeerHandle)> {
        self.inner
            .iter()
            .filter(|e| e.value().account_id == account_id)
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect()
    }

    pub fn get(&self, device_id: &str) -> Option<crate::ws::PeerHandle> {
        self.inner.get(device_id).map(|e| e.value().clone())
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
        .route("/ws", axum::routing::get(ws::ws_handler))
        .with_state(state)
}

/// 集成测试基建：自定义 keepalive 起服务器
pub async fn spawn_test_server_with(
    keepalive: crate::ws::Keepalive,
) -> (SocketAddr, Arc<AppState>) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("t.db");
    let db = db::open(db_path.to_str().unwrap()).unwrap();
    let state = Arc::new(AppState {
        db: std::sync::Mutex::new(db),
        jwt_secret: rand::random(),
        presence: PresenceRegistry::new(),
        keepalive,
    });
    let app = build_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // TempDir 必须活到服务器任务结束（tempfile 的约定：提前 drop 会在 Windows 上把
    // 仍在使用的 DB 文件标记为删除挂起）。服务器存活期间目录必须保持有效。
    tokio::spawn(async move {
        let _keep = dir;
        axum::serve(listener, app).await.unwrap();
    });
    (addr, state)
}

/// 集成测试基建：随机端口起服务器
pub async fn spawn_test_server() -> (SocketAddr, Arc<AppState>) {
    spawn_test_server_with(crate::ws::Keepalive::default()).await
}

/// oneshot 测试用：不起端口
pub async fn test_state() -> Arc<AppState> {
    let db = db::open(":memory:").unwrap();
    Arc::new(AppState {
        db: std::sync::Mutex::new(db),
        jwt_secret: rand::random(),
        presence: PresenceRegistry::new(),
        keepalive: crate::ws::Keepalive::default(),
    })
}

/// 集成测试基建:随机端口起 TLS 服务器,返回 (地址, ca.crt 路径, 存活目录)。
/// TempDir 持有证书与 DB,存活期须覆盖整个测试。
pub async fn spawn_test_tls_server() -> (SocketAddr, std::path::PathBuf, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    tls::generate_dev_tls(dir.path()).unwrap();
    let db = db::open(dir.path().join("t.db").to_str().unwrap()).unwrap();
    let state = Arc::new(AppState {
        db: std::sync::Mutex::new(db),
        jwt_secret: rand::random(),
        presence: PresenceRegistry::new(),
        keepalive: crate::ws::Keepalive::default(),
    });
    let app = build_router(state);
    tls::ensure_provider();
    let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(
        dir.path().join("server.crt"),
        dir.path().join("server.key"),
    )
    .await
    .unwrap();
    let handle = axum_server::Handle::new();
    let h = handle.clone();
    // TempDir 不可 clone(tempfile 3.x):目录存活由返回的 TempDir 保证(见上文档)。
    tokio::spawn(async move {
        axum_server::bind_rustls(SocketAddr::from(([127, 0, 0, 1], 0)), config)
            .handle(h)
            .serve(app.into_make_service())
            .await
            .unwrap();
    });
    loop {
        if let Some(a) = handle.listening().await {
            return (a, dir.path().join("ca.crt"), dir);
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
