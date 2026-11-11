//! 池内 mux 直连的标签分派 serve(V3-5 Stage B Task 3)。
//!
//! 协商出 mux 的入池直连上,每条双向流以一个流类型标签字节开头
//! (`transport::mux` 单一真源):[`TAG_CTRL`] → ctrl 请求-响应(本任务
//! 立臂:[`read_request_frame`] → [`handle_ctrl_request`] →
//! [`write_response_frame`],与裸 serve 同一分发,仅帧前多一字节标签);
//! [`TAG_TRANSFER`] → 传输引擎控制流(T4:经 [`XferSlots`] 槽登记簿按
//! 设备键 rendezvous,连接克隆 + 流两半移交接收端暖传输轮;无槽/接收方
//! 已弃 → warn + 丢弃该流,不挂死、不崩连接)。未识别字节 warn+drop
//! 单流(裸帧误入 mux 连接即按此显形,连接与其余流不受累)。
//!
//! **连接所有权纪律**(T2 实测教训):accept 循环本体持 conn(参数拥
//! 有),spawn 的流任务只拿流两半(tx/rx)——绝不持连接克隆在任务结束
//! 时触发隐式 CONNECTION_CLOSE 作废连接上其余在途帧。连接关闭由对端
//! 或本端驱逐/reset 显式驱动,serve 随 accept_bi 失败自然收尾。
//! TAG_TRANSFER 的移交是唯一例外:接收端暖传输轮需要连接本体做
//! `accept_uni` 块流与对端证书读取,移交的是 conn.clone()(引用计数
//! 句柄,轮终局 drop 不关连接),accept 循环仍持原柄。

use crate::state::DaemonState;
use mtdss_core::transport::ctrl::{read_request_frame, write_response_frame};
use mtdss_core::transport::mux::{TAG_CTRL, TAG_TRANSFER};
use mtdss_proto::ctrl::{CtrlRequest, CtrlResponse};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

/// 暖传输轮 handoff 等待上限(spec §3.2 有界等待):接收端回 warm_ok=true
/// 后在槽上等 TAG_TRANSFER 控制流移交——发送端 warm 路径无 STUN/打洞,
/// Accept 后只剩 open_bi + 写清单,15s 足够;超时按等待阶段失败收口
/// (匿名卡 failed / 下载绑定轮保持 running),常驻循环绝不挂死
/// (Review Focus 3:warm_ok=true 后流永不到达,靠发送端重试自愈)。
pub(crate) const WARM_XFER_HANDOFF_TIMEOUT: Duration = Duration::from_secs(15);

/// mux serve → 接收端暖传输轮的移交体:暖连接本体(接收引擎做 accept_uni
/// 块流与对端证书读取;克隆句柄,轮终局 drop 不关连接——见模块头所有权
/// 纪律)+ 已读掉 TAG_TRANSFER 标签的控制流两半(引擎从 manifest 读起
/// 照常推进,`transfer::recv_files_on_ctl`)。
pub(crate) struct XferHandoff {
    pub(crate) conn: quinn::Connection,
    pub(crate) ctl: (quinn::SendStream, quinn::RecvStream),
}

/// 暖传输槽登记簿(V3-5 Stage B T4 rendezvous):接收端常驻循环识别
/// warm_reuse 来件后 `register`(按设备键单槽,占用返 None——同设备并发
/// 第二暖来件据此回 warm_ok:false 走打洞);mux serve 的 TAG_TRANSFER 臂
/// `take` 取走 Sender 移交连接+流(取走即清位)。生命周期闭环:等待方
/// 超时/取消时主动 take 清位;Sender 被 take 后 send 到已弃 Receiver 返
/// Err 自然丢弃——任一路径不泄漏、不留陈旧槽(迟到的移交流撞空槽按
/// warn+drop 收口)。键是**设备**而非连接:发送端开流的连接在接收端可能
/// 处于 decline 限时服务窗(双侧并发拨的竞态分歧窗,Review Focus 2),
/// 任一 mux 连接上的移交都命中同一槽。
#[derive(Default)]
pub(crate) struct XferSlots(Mutex<HashMap<String, oneshot::Sender<XferHandoff>>>);

impl XferSlots {
    /// 登记等待槽:该设备已有在等暖轮(占用)→ None;空位 → Some(接收
    /// 端在其上等 handoff)。std 锁(不跨 await,register/take 均即返)。
    pub(crate) fn register(&self, device: &str) -> Option<oneshot::Receiver<XferHandoff>> {
        let mut slots = self.0.lock().unwrap();
        if slots.contains_key(device) {
            return None;
        }
        let (tx, rx) = oneshot::channel();
        slots.insert(device.to_string(), tx);
        Some(rx)
    }

    /// 取走该设备的移交 Sender(serve 侧 TAG_TRANSFER 臂消费;等待方轮
    /// 终局清位同走这里)。取走即清位——同键再 take 必 None。
    pub(crate) fn take(&self, device: &str) -> Option<oneshot::Sender<XferHandoff>> {
        self.0.lock().unwrap().remove(device)
    }
}

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
                        // 传输引擎控制流(T4):槽登记簿按设备键 rendezvous
                        // ——take 等待中的接收端暖传输轮,把连接克隆 + 流两半
                        // 移交(XferHandoff;conn 是引用计数克隆,轮终局 drop
                        // 不关连接,accept 循环仍持原柄——所有权纪律见模块头)。
                        // 接收方已弃(超时/取消清位后)→ send Err → warn 丢弃
                        // (流两半离开作用域即关,发起端读侧见流终止);无槽
                        // (未注册/已被取走)→ warn + drop 流——两路都不挂死、
                        // 不崩连接,其余流不受累。
                        match state.xfer_slots().take(&device) {
                            Some(handoff_tx) => {
                                if handoff_tx
                                    .send(XferHandoff {
                                        conn: conn.clone(),
                                        ctl: (tx, rx),
                                    })
                                    .is_err()
                                {
                                    tracing::warn!(
                                        device = %device,
                                        "mux TAG_TRANSFER 移交时接收轮已弃,丢弃"
                                    );
                                }
                            }
                            None => {
                                tracing::warn!(
                                    device = %device,
                                    "mux TAG_TRANSFER 流到达但无等待槽,丢弃该流"
                                );
                            }
                        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 槽登记簿生命周期(V3-5 Stage B T4 Step 1):register Some → 占用
    /// (再 register None)→ take Some(取走即清位)→ 再 take None →
    /// 清位后 register 复用又 Some。
    #[test]
    fn xfer_slots_register_take_occupy() {
        let slots = XferSlots::default();
        assert!(slots.register("d").is_some(), "空闲槽 register 必须 Some");
        assert!(slots.register("d").is_none(), "已占用槽 register 必须 None");
        assert!(slots.take("d").is_some(), "占用槽 take 必须 Some");
        assert!(slots.take("d").is_none(), "取走即清位,再 take 必须 None");
        assert!(
            slots.register("d").is_some(),
            "清位后槽可复用,register 又 Some"
        );
    }

    /// handoff 等待上限锚(spec §3.2 有界等待):改值须过本断言。
    #[test]
    fn handoff_timeout_anchor() {
        assert_eq!(WARM_XFER_HANDOFF_TIMEOUT, Duration::from_secs(15));
    }
}
