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
//! **连接所有权纪律**(T2 实测教训,V3-6 Task 2 起任务形态升级):serve
//! 循环本体持 conn 原柄(参数拥有),唯一退场路径是 accept_bi 失败(对
//! 端关/本端驱逐显式 close)——届时连接在协议层已亡。每条已 accept 流
//! spawn 一个分派任务([`dispatch_stream`]),任务持 conn 引用计数克隆
//! (TAG_TRANSFER 臂移交需要,标签读前无从分派)+ 流两半:serve 本体在
//! 世时任务终局的 drop 只减引用计数,绝不触发隐式 CONNECTION_CLOSE 作废
//! 连接上其余在途帧;serve 退场即连接已亡,残余任务的克隆 drop 无作废面。
//! TAG_TRANSFER 的移交仍是唯一把克隆交给**长生命周期接收方**(暖传输轮
//! 做 `accept_uni` 块流与对端证书读取)的例外,移交的是 conn.clone()(
//! 引用计数句柄,轮终局 drop 不关连接),accept 循环仍持原柄。

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
/// serve 与响应端直连臂/decline 臂统一过它进这里)。accept_bi 循环把每
/// 条已 accept 流连同连接克隆、状态、设备键 spawn 给 [`dispatch_stream`]
/// (标签读 + 分派随各自流任务,一条 stall 流只停自己——accept 循环不
/// 内联等待任何一条流,V3-6 消队头阻塞);连接关闭(对端关/本端驱逐
/// 显式 close)即返回。
pub(crate) async fn serve_conn_mux(
    conn: quinn::Connection,
    state: Arc<DaemonState>,
    device: String,
) {
    loop {
        match conn.accept_bi().await {
            Ok((tx, rx)) => {
                // 每流一个任务(V3-6 Task 2 消队头阻塞):标签读 + 分派
                // 整体入 spawn——对端开流后迟迟不写标签字节,只停该流自
                // 己的任务里,accept 循环立即回位 accept 下一条流;旧形态
                // 内联 await read_tag,一条 stall 流即冻结连接上全部后续
                // 流的分派。spawn 面由对端开流速率天然限界(与原 TAG_CTRL
                // 臂的 spawn 同口径,无新增无界风险)。
                tokio::spawn(dispatch_stream(
                    conn.clone(),
                    tx,
                    rx,
                    state.clone(),
                    device.clone(),
                ));
            }
            // 连接关闭(对端退出/网络断/本端驱逐显式 close),serve 收尾
            Err(_) => return,
        }
    }
}

/// 单条已 accept 流的标签读 + 分派任务(V3-6 Task 2 自 serve 循环拆出):
/// 读首字节标签后原地分派,语义与旧内联形态逐分支一致——标签读失败即弃
/// 整条流(不进分派,连接与其余流不受累);TAG_TRANSFER 的槽 take/移交
/// 逐字节保持。conn 是引用计数克隆:serve 本体持原柄在世,本任务终局的
/// drop 只减引用计数,绝不触发隐式 CONNECTION_CLOSE(所有权纪律见模块头);
/// 仅 TAG_TRANSFER 臂把再克隆的一柄随移交交出。
async fn dispatch_stream(
    conn: quinn::Connection,
    tx: quinn::SendStream,
    rx: quinn::RecvStream,
    state: Arc<DaemonState>,
    device: String,
) {
    let (tag, tx, rx) = match read_tag(tx, rx).await {
        Some(v) => v,
        None => return,
    };
    match tag {
        TAG_CTRL => {
            // ctrl 请求-响应:与裸 serve 同一分发(handle_ctrl_request),
            // 帧读写换标签版原语;本任务即该流的任务体,原地 await——
            // 每流一任务不变。
            handle_ctrl_stream(tx, rx, state).await;
        }
        TAG_TRANSFER => {
            // 传输引擎控制流(T4):槽登记簿按设备键 rendezvous——take
            // 等待中的接收端暖传输轮,把连接克隆 + 流两半移交(XferHandoff;
            // conn 是引用计数克隆,轮终局 drop 不关连接,accept 循环仍持
            // 原柄——所有权纪律见模块头)。接收方已弃(超时/取消清位后)
            // → send Err → warn 丢弃(流两半离开作用域即关,发起端读侧
            // 见流终止);无槽(未注册/已被取走)→ warn + drop 流——两路
            // 都不挂死、不崩连接,其余流不受累。
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

/// 读流首字节标签:失败(对端开流即断/零字节)静默弃流(debug 留痕),
/// 返回 None 让流任务收尾——单条坏流不作废连接,也不影响连接上其余流
/// (V3-6 起标签读随各自流任务进行,读失败只收束本流任务)。
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
    use prost::Message;
    use std::net::SocketAddr;

    /// 回环真握手取一对连接句柄(relaymem 同款:B 服务端白名单 A,A pin
    /// B 公钥)→(客户端侧, 服务端侧)。
    async fn loopback_pair() -> (quinn::Connection, quinn::Connection) {
        use mtdss_core::transport::endpoint::{
            accept_pinned, connect_pinned, make_endpoint, make_server_endpoint,
        };
        let a = mtdss_core::identity::DeviceIdentity::generate();
        let b = mtdss_core::identity::DeviceIdentity::generate();
        let allowed: HashMap<String, ed25519_dalek::VerifyingKey> =
            [(a.device_id.clone(), a.public_key())].into_iter().collect();
        let ep_b =
            make_server_endpoint(SocketAddr::from(([127, 0, 0, 1], 0)), &b, &allowed).unwrap();
        let addr = ep_b.local_addr().unwrap();
        let ep_a = make_endpoint(SocketAddr::from(([127, 0, 0, 1], 0))).unwrap();
        let server = tokio::spawn(async move { accept_pinned(&ep_b, &allowed).await.unwrap().0 });
        let client = connect_pinned(&ep_a, addr, &b.public_key(), &a).await.unwrap();
        (client, server.await.unwrap())
    }

    /// 测试态:独立临时 home(ctrl.rs tests 同款装配面单点)——测试只走
    /// body:None 请求的结构化错误响应,不触盘,home 生命周期到函数尾即弃。
    fn test_state() -> Arc<DaemonState> {
        let home = tempfile::tempdir().unwrap();
        let (events, _) = tokio::sync::broadcast::channel(16);
        // 更新服务(Task 11):缺省失败安全配置(.invalid/无钥),不触更新
        // 面;非 Windows 为单元占位。
        #[cfg(windows)]
        let update = std::sync::Arc::new(crate::update::UpdateService::new(
            crate::update::UpdateConfig::default(),
            events.clone(),
            tokio::sync::watch::channel(false).0,
        ));
        #[cfg(not(windows))]
        let update = ();
        Arc::new(
            DaemonState::load(
                home.path(),
                home.path().join("inbox"),
                None,
                crate::inbox_dest::staging_dir(home.path()),
                false,
                false,
                Default::default(),
                Default::default(),
                crate::ctrl::ctrl_default_tuning(),
                events,
                update,
            )
            .unwrap(),
        )
    }

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

    /// 队头阻塞消(V3-6 计划 Task 2):同一条 mux 连接上先开一条不写
    /// 任何字节的流(stall;QUIC 隐式开流——后开流的帧一到,quinn 即把
    /// 更小编号的 stall 流当已开交付 accept_bi),随后开第二条 TAG_CTRL
    /// 流——spawn 化前 serve 循环内联 await read_tag 恒停 stall 流上,
    /// 第二条永不分派(响应 5s 超时即红);修后标签读随各自流任务进行,
    /// 第二条立即分派往返(绿)。断言面 = 响应帧可读(observable),
    /// 无固定 sleep 过关路径。
    #[tokio::test]
    async fn stalled_tagless_stream_does_not_block_next_ctrl_stream() {
        let (client, server) = loopback_pair().await;
        // serve 本体持有连接跑 accept_bi 循环(参数拥有;测试不另持句柄,
        // 免隐式 CONNECTION_CLOSE 作废在途响应帧——所有权纪律见模块头)
        tokio::spawn(serve_conn_mux(server, test_state(), "peer-a".into()));

        // 流 A(流号小):开流即 stall——一个字节不写、不半关
        let (_tx_a, _rx_a) = client.open_bi().await.unwrap();

        // 流 B(流号大):TAG_CTRL + 合法请求帧(body:None → 结构化错误
        // 响应,秒回且不触盘、不依赖会话)
        let (mut tx_b, mut rx_b) = client.open_bi().await.unwrap();
        tx_b.write_all(&[TAG_CTRL]).await.unwrap();
        let req = CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: None,
        };
        let body = req.encode_to_vec();
        tx_b
            .write_all(&(body.len() as u32).to_le_bytes())
            .await
            .unwrap();
        tx_b.write_all(&body).await.unwrap();
        let _ = tx_b.finish();

        // observable:B 的响应帧在时限内可读(4 字节长度前缀 + 帧体)。
        // 现状 serve 堵 stall 流,读不满 → 超时 panic(红);修后毫秒达(绿)。
        let resp = tokio::time::timeout(Duration::from_secs(5), async {
            let mut len_buf = [0u8; 4];
            rx_b.read_exact(&mut len_buf).await.unwrap();
            let len = u32::from_le_bytes(len_buf) as usize;
            let mut buf = vec![0u8; len];
            rx_b.read_exact(&mut buf).await.unwrap();
            CtrlResponse::decode(buf.as_slice()).unwrap()
        })
        .await
        .expect("stall 流不得堵死后续 TAG_CTRL 流:B 的响应超时未达");
        assert_eq!(resp.version, mtdss_proto::CTRL_VERSION);
    }
}
