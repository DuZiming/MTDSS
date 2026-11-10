//! 池内 mux 直连的标签分派 serve(V3-5 Stage B Task 3)。
//!
//! 协商出 mux 的入池直连上,每条双向流以一个流类型标签字节开头
//! (`transport::mux` 单一真源):[`TAG_CTRL`] → ctrl 请求-响应(本任务
//! 立臂:[`read_request_frame`] → [`handle_ctrl_request`] →
//! [`write_response_frame`],与裸 serve 同一分发,仅帧前多一字节标签);
//! [`TAG_TRANSFER`] → 传输引擎控制流(**T4 接管**:槽登记簿 rendezvous
//! 移交接收端暖传输轮;本任务占位 warn+drop,不挂死、不崩连接)。
//! 未识别字节 warn+drop 单流(裸帧误入 mux 连接即按此显形,连接与
//! 其余流不受累)。
//!
//! **连接所有权纪律**(T2 实测教训):accept 循环本体持 conn(参数拥
//! 有),spawn 的流任务只拿流两半(tx/rx)——绝不持连接克隆在任务结束
//! 时触发隐式 CONNECTION_CLOSE 作废连接上其余在途帧。连接关闭由对端
//! 或本端驱逐/reset 显式驱动,serve 随 accept_bi 失败自然收尾。

use crate::state::DaemonState;
use mtdss_core::transport::ctrl::{read_request_frame, write_response_frame};
use mtdss_core::transport::mux::{TAG_CTRL, TAG_TRANSFER};
use mtdss_proto::ctrl::{CtrlRequest, CtrlResponse};
use std::sync::Arc;

/// mux 直连的标签分派 serve(挂载点见 ctrl::serve_pooled,发起端双职责
/// serve 与响应端直连臂/decline 臂统一过它进这里)。accept_bi 循环读
/// 首字节标签分派;连接关闭(对端关/本端驱逐显式 close)即返回。
pub(crate) async fn serve_conn_mux(
    conn: quinn::Connection,
    state: Arc<DaemonState>,
    device: String,
) {
    loop {
        match conn.accept_bi().await {
            Ok((tx, rx)) => {
                // 首字节标签在本体读(流任务 spawn 前定分派,标签读失败
                // 即弃整条流,不进任务)
                let (tag, tx, rx) = match read_tag(tx, rx).await {
                    Some(v) => v,
                    None => continue,
                };
                match tag {
                    TAG_CTRL => {
                        // ctrl 请求-响应:与裸 serve 同一分发(handle_ctrl_
                        // request),帧读写换标签版原语;流任务只拿流两半
                        // (连接所有权纪律,见模块头)。
                        let st = state.clone();
                        tokio::spawn(handle_ctrl_stream(tx, rx, st));
                    }
                    TAG_TRANSFER => {
                        // T4 接管(槽登记簿按设备键移交接收端暖传输轮);
                        // 本任务占位:warn + drop 流(两半离开作用域即关,
                        // 发起端读侧见流终止,不挂死、不崩连接)。
                        tracing::warn!(
                            device = %device,
                            "mux TAG_TRANSFER 流到达(T4 前占位),丢弃"
                        );
                    }
                    other => {
                        tracing::warn!(device = %device, tag = other,
                            "mux 连接上未识别的流标签,丢弃该流");
                    }
                }
            }
            // 连接关闭(对端退出/网络断/本端驱逐显式 close),serve 收尾
            Err(_) => return,
        }
    }
}

/// 读流首字节标签:失败(对端开流即断/零字节)静默弃流(debug 留痕),
/// 返回 None 让 accept 循环继续——单条坏流不作废连接。
async fn read_tag(
    tx: quinn::SendStream,
    mut rx: quinn::RecvStream,
) -> Option<(u8, quinn::SendStream, quinn::RecvStream)> {
    let mut tag = [0u8; 1];
    match rx.read_exact(&mut tag).await {
        Ok(()) => Some((tag[0], tx, rx)),
        Err(e) => {
            tracing::debug!("mux 流读首字节标签失败,丢弃该流: {e}");
            None
        }
    }
}

/// 单条 ctrl 请求流的处理任务(裸 serve 的 handle_stream 同款舞步,帧
/// 读写换标签版原语):读请求帧 → handle_ctrl_request → 写响应帧 →
/// 半关;请求读不出/响应写不出静默丢弃该流,不影响连接上的其余请求。
async fn handle_ctrl_stream(
    mut tx: quinn::SendStream,
    mut rx: quinn::RecvStream,
    state: Arc<DaemonState>,
) {
    let req: CtrlRequest = match read_request_frame(&mut rx).await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!("mux ctrl 请求读失败,丢弃流: {e}");
            return;
        }
    };
    let resp: CtrlResponse = crate::ctrl::handle_ctrl_request(state, req).await;
    if let Err(e) = write_response_frame(&mut tx, &resp).await {
        tracing::debug!("mux ctrl 响应写失败: {e}");
        return;
    }
    let _ = tx.finish();
}
