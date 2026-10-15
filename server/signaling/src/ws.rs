use axum::extract::ws::Message;

/// 在线设备的发送端
#[derive(Clone)]
pub struct PeerHandle {
    pub account_id: String,
    pub tx: tokio::sync::mpsc::Sender<Message>,
}

pub async fn ws_handler() -> &'static str {
    "implemented in task 6"
}
