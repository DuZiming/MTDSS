//! 中继数据面集成测试:真 UdpSocket 互发、NAT 重绑、畸形输入、记账落库、
//! 配额杀会话、月界滚动、巡检热调/停用杀。按计划钉死的数据报算法逐条钉住。

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::Message;
use futures_util::{SinkExt, StreamExt};
use mtdss_proto::ws::{
    ws_client_frame, ws_server_frame, Hello, RelayAllocate, WsClientFrame, WsServerFrame,
};
use prost::Message as _;
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use signaling::relay::ticket::{mint_ticket, Session, TokenBucket};
use signaling::relay::{spawn_relay, sweep_refresh};
use signaling::ws::PeerHandle;
use signaling::AppState;

/// 在随机端口起中继数据面,返回测试可达的回环地址(bind 0.0.0.0,测试走 127.0.0.1)。
async fn spawn_relay_on(state: &Arc<AppState>) -> std::net::SocketAddr {
    spawn_relay(state.clone(), 0, "127.0.0.1:0".into()).unwrap();
    let relay = state.relay.get().unwrap();
    let port = relay.socket.local_addr().unwrap().port();
    std::net::SocketAddr::from(([127, 0, 0, 1], port))
}

/// 预置账号并开通中继(sweep 巡检按 enabled 判杀,测试账号须启用)。
fn prep_account(state: &Arc<AppState>, email: &str) -> String {
    signaling::admin::user_add(&state.db.lock().unwrap(), email).unwrap();
    signaling::admin::relay_set_enabled(&state.db.lock().unwrap(), email, true).unwrap();
    state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT id FROM accounts WHERE email=?1", [email], |r| r.get(0))
        .unwrap()
}

/// 用服务器自己的 secret 铸票并插入注册表,返回 (sid, tag_a, tag_b)。
/// rate_kbps 传 0 = 不限(数据面对 0 跳过会话桶);slot_addrs 可预设槽位
/// 跳过自然握手(记账/限速等测试要精确控制线上首包)。
fn insert_session_at(
    state: &Arc<AppState>,
    seed: u64,
    account: &str,
    rate_kbps: u32,
    slot_addrs: [Option<std::net::SocketAddr>; 2],
) -> (u64, [u8; 16], [u8; 16]) {
    let relay = state.relay.get().unwrap();
    let expires_at = signaling::db::now() + 3_600;
    let (ticket, tag_a, tag_b) =
        mint_ticket(&relay.secret, expires_at, &mut StdRng::seed_from_u64(seed));
    let session = Session {
        ticket,
        account_id: account.into(),
        rate_kbps,
        slot_addrs,
        bucket_a: TokenBucket::new(rate_kbps),
        bucket_b: TokenBucket::new(rate_kbps),
        bytes_accounted: 0,
        last_refresh: std::time::Instant::now(),
    };
    relay.registry.lock().unwrap().insert(session, tag_a, tag_b);
    (ticket.sid, tag_a, tag_b)
}

fn insert_session(
    state: &Arc<AppState>,
    seed: u64,
    account: &str,
    rate_kbps: u32,
) -> (u64, [u8; 16], [u8; 16]) {
    insert_session_at(state, seed, account, rate_kbps, [None, None])
}

/// 绑定合成对端并 connect 到中继(只收来自中继端口的回包)。
async fn bind_peer(relay: &std::net::SocketAddr) -> tokio::net::UdpSocket {
    let s = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    s.connect(relay).await.unwrap();
    s
}

/// 发一个带标签的数据报。
async fn send(peer: &tokio::net::UdpSocket, tag: &[u8; 16], payload: &[u8]) {
    let mut d = Vec::with_capacity(16 + payload.len());
    d.extend_from_slice(tag);
    d.extend_from_slice(payload);
    peer.send(&d).await.unwrap();
}

/// 收一个数据报(3s 超时)。
async fn recv(peer: &tokio::net::UdpSocket) -> Vec<u8> {
    let mut buf = [0u8; 65_535];
    let n = tokio::time::timeout(Duration::from_secs(3), peer.recv(&mut buf))
        .await
        .unwrap()
        .unwrap();
    buf[..n].to_vec()
}

/// 轮询等待条件成立:转发完成只同步到 send_to,记账/累计是其后的同步尾巴,
/// 测试侧须让出线程等它跑完(当前线程 runtime 下 sleep 即让出)。
async fn wait_until(mut cond: impl FnMut() -> bool, what: &str) {
    for _ in 0..200 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not met: {what}");
}

/// 自然槽位就绪(线上客户端先互发保活的等价物):A 先发只绑 A 槽(对侧
/// 地址未知即丢,B 收不到),B 随后发即绑 B 槽并被转发到 A;A 收到握手包
/// 即两槽就绪、双向转发畅通。
async fn handshake(
    a: &tokio::net::UdpSocket,
    tag_a: &[u8; 16],
    b: &tokio::net::UdpSocket,
    tag_b: &[u8; 16],
) {
    send(a, tag_a, b"hs-a").await; // 只绑 A 槽,对侧未知 → 丢
    send(b, tag_b, b"hs-b").await; // 绑 B 槽,对侧=A → 转发到 A
    assert_eq!(recv(a).await, b"hs-b");
}

#[tokio::test]
async fn relay_forwards_both_directions() {
    let (_addr, state) = signaling::spawn_test_server().await;
    let relay_addr = spawn_relay_on(&state).await;
    let acc = prep_account(&state, "fwd@x.y");
    let (sid, tag_a, tag_b) = insert_session(&state, 1, &acc, 0);
    let a = bind_peer(&relay_addr).await;
    let b = bind_peer(&relay_addr).await;

    handshake(&a, &tag_a, &b, &tag_b).await;
    // 双向各一包:载荷 strip 16B 头后原样到达
    send(&a, &tag_a, b"hello-b").await;
    assert_eq!(recv(&b).await, b"hello-b");
    send(&b, &tag_b, b"hi-a").await;
    assert_eq!(recv(&a).await, b"hi-a");

    // 会话仍在册,字节按整报文含头双向累计:
    // B 握手 16+4 + A→B 16+7 + B→A 16+4(A 的握手只绑槽,未转发不记账)
    wait_until(
        || {
            state.relay.get().unwrap().registry.lock().unwrap().get_mut(sid).unwrap().bytes_accounted
                == (20 + 23 + 20) as u64
        },
        "bytes_accounted",
    )
    .await;
}

#[tokio::test]
async fn rebind_new_source_address_keeps_flow() {
    let (_addr, state) = signaling::spawn_test_server().await;
    let relay_addr = spawn_relay_on(&state).await;
    let acc = prep_account(&state, "rebind@x.y");
    let (_sid, tag_a, tag_b) = insert_session(&state, 2, &acc, 0);
    let b = bind_peer(&relay_addr).await;

    // 首包:A 经 socket1 上线并完成槽位就绪
    let a1 = bind_peer(&relay_addr).await;
    handshake(&a1, &tag_a, &b, &tag_b).await;
    drop(a1);

    // 裁定 D:NAT 重绑——A 换新端口(新 UdpSocket)再发,仍被认领并转发
    let a2 = bind_peer(&relay_addr).await;
    send(&a2, &tag_a, b"from-new-port").await;
    assert_eq!(recv(&b).await, b"from-new-port");

    // 回包必须追到 A 的新地址(旧地址已随 socket1 消失,能收到即重绑生效)
    send(&b, &tag_b, b"back-to-new").await;
    assert_eq!(recv(&a2).await, b"back-to-new");
}

#[tokio::test]
async fn malformed_datagrams_dropped_silently() {
    let (_addr, state) = signaling::spawn_test_server().await;
    let relay_addr = spawn_relay_on(&state).await;
    let acc = prep_account(&state, "mal@x.y");
    let (_sid, tag_a, tag_b) = insert_session(&state, 3, &acc, 0);
    let a = bind_peer(&relay_addr).await;
    let b = bind_peer(&relay_addr).await;
    handshake(&a, &tag_a, &b, &tag_b).await;

    // 超短报文(<16B 无完整头)
    a.send(b"short").await.unwrap();
    // 随机 16B 伪标签(必有别于真标签)
    let mut forged = [0u8; 24];
    StdRng::seed_from_u64(999).fill_bytes(&mut forged);
    assert_ne!(&forged[..16], &tag_a[..]);
    a.send(&forged).await.unwrap();

    // 畸形输入静默丢弃:后续正常会话不受扰,任务不崩
    send(&a, &tag_a, b"still-alive").await;
    assert_eq!(recv(&b).await, b"still-alive");
    send(&b, &tag_b, b"me-too").await;
    assert_eq!(recv(&a).await, b"me-too");
}

#[tokio::test]
async fn accounting_counts_whole_datagrams_both_directions() {
    let (_addr, state) = signaling::spawn_test_server().await;
    let relay_addr = spawn_relay_on(&state).await;
    let acc = prep_account(&state, "acc@x.y");
    let a = bind_peer(&relay_addr).await;
    let b = bind_peer(&relay_addr).await;
    // 预设槽位:跳过握手,让线上字节只含本测试的报文
    let (_sid, tag_a, tag_b) = insert_session_at(
        &state,
        4,
        &acc,
        0,
        [Some(a.local_addr().unwrap()), Some(b.local_addr().unwrap())],
    );

    // 双向各一包:线上字节 = 载荷 + 16B 头(裁定 H,双向同价)
    send(&a, &tag_a, &[0u8; 100]).await; // 线上 116
    assert_eq!(recv(&b).await.len(), 100);
    send(&b, &tag_b, &[1u8; 200]).await; // 线上 216
    assert_eq!(recv(&a).await.len(), 200);
    wait_until(
        || state.relay.get().unwrap().quota.lock().unwrap().dirty_total() > 0,
        "ledger charged",
    )
    .await;

    // 手动触发刷库(数据面另有 1 MiB/5s 自动阈值,测试不等待)
    let period = signaling::admin::current_period();
    {
        let mut ledger = state.relay.get().unwrap().quota.lock().unwrap();
        let conn = state.db.lock().unwrap();
        ledger.flush(&conn).unwrap();
    }
    let used: i64 = state.db.lock().unwrap().query_row(
        "SELECT bytes_forwarded FROM relay_usage WHERE account_id=?1 AND period=?2",
        rusqlite::params![acc, period],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(used, 116 + 216, "当期 relay_usage 须等于线上总字节(含头)");
}

#[tokio::test]
async fn quota_exhaustion_kills_and_notifies_both_ends() {
    let (_addr, state) = signaling::spawn_test_server().await;
    let relay_addr = spawn_relay_on(&state).await;
    let acc = prep_account(&state, "quota@x.y");
    let a = bind_peer(&relay_addr).await;
    let b = bind_peer(&relay_addr).await;
    // 预设槽位:首包即转发并触发台账 seed
    let (sid, tag_a, _tag_b) = insert_session_at(
        &state,
        5,
        &acc,
        0,
        [Some(a.local_addr().unwrap()), Some(b.local_addr().unwrap())],
    );

    // 假 peer handle:杀会话通告走 presence 推送,挂 mpsc 直接验收
    let (tx_a, mut rx_a) = tokio::sync::mpsc::channel::<Message>(8);
    let (tx_b, mut rx_b) = tokio::sync::mpsc::channel::<Message>(8);
    state.presence.insert("dev-a", PeerHandle { account_id: acc.clone(), tx: tx_a });
    state.presence.insert("dev-b", PeerHandle { account_id: acc.clone(), tx: tx_b });

    send(&a, &tag_a, &[0u8; 10]).await; // 首包触发台账 seed
    let _ = recv(&b).await;
    wait_until(
        || state.relay.get().unwrap().quota.lock().unwrap().known(&acc),
        "ledger seeded",
    )
    .await;

    // 注入 1KB 余量(裁定:直改台账字段,不绕真实配置/真实时钟)
    state.relay.get().unwrap().quota.lock().unwrap().accounts.get_mut(&acc).unwrap().remaining = 1024;

    // 灌包(每包线上 326B)直至超限被杀
    for _ in 0..10 {
        send(&a, &tag_a, &[7u8; 310]).await;
    }

    // 双端各收到一条 RelaySession{sid, alive:false}
    for rx in [&mut rx_a, &mut rx_b] {
        let msg = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let Message::Binary(bin) = msg else { panic!("expected binary frame") };
        let f = WsServerFrame::decode(bin.as_ref()).unwrap();
        match f.frame {
            Some(ws_server_frame::Frame::RelaySession(rs)) => {
                assert_eq!(rs.sid, sid, "通告须携带被杀会话号");
                assert!(!rs.alive, "配额杀会话须通告 alive:false");
            }
            other => panic!("expected relay session frame, got {other:?}"),
        }
    }

    // 会话已摘除:后续同标签数据报无人认领,静默丢弃且不崩
    send(&a, &tag_a, b"ghost").await;
    assert!(state.relay.get().unwrap().registry.lock().unwrap().lookup(&tag_a).is_none());
}

#[tokio::test]
async fn month_rollover_resets_accounting_from_zero() {
    let (_addr, state) = signaling::spawn_test_server().await;
    let relay_addr = spawn_relay_on(&state).await;
    let acc = prep_account(&state, "roll@x.y");
    let a = bind_peer(&relay_addr).await;
    let b = bind_peer(&relay_addr).await;
    // 预设槽位:线上字节只含本测试注入的两包,月界对账才精确
    let (_sid, tag_a, _tag_b) = insert_session_at(
        &state,
        6,
        &acc,
        0,
        [Some(a.local_addr().unwrap()), Some(b.local_addr().unwrap())],
    );

    // 注入"上个月"作为台账当期(裁定:直改 period 字段;巡检比对真实钟后滚动)
    state.relay.get().unwrap().quota.lock().unwrap().period = "2000-01".into();
    send(&a, &tag_a, &[0u8; 100]).await; // 线上 116,记入 2000-01
    let _ = recv(&b).await;
    wait_until(
        || state.relay.get().unwrap().quota.lock().unwrap().dirty_total() > 0,
        "ledger charged",
    )
    .await;

    // 巡检一次:发现台账 period 落后 → 旧期尾巴按旧 period 落库 + 切当期 + 余量重认
    sweep_refresh(&state).unwrap();
    assert_eq!(
        state.relay.get().unwrap().quota.lock().unwrap().period,
        signaling::admin::current_period(),
        "巡检后台账 period 须滚动到当前周期"
    );

    // 新周期继续传:落库进新 period 行,从零起算(不背旧期余额)
    send(&a, &tag_a, &[1u8; 200]).await; // 线上 216
    let _ = recv(&b).await;
    wait_until(
        || {
            let ledger = state.relay.get().unwrap().quota.lock().unwrap();
            ledger.accounts.get(&acc).map(|q| q.dirty) == Some(216)
        },
        "new period charge",
    )
    .await;
    {
        let mut ledger = state.relay.get().unwrap().quota.lock().unwrap();
        let conn = state.db.lock().unwrap();
        ledger.flush(&conn).unwrap();
    }

    let conn = state.db.lock().unwrap();
    let old: i64 = conn.query_row(
        "SELECT bytes_forwarded FROM relay_usage WHERE account_id=?1 AND period='2000-01'",
        [&acc],
        |r| r.get(0),
    ).unwrap();
    let new: i64 = conn.query_row(
        "SELECT bytes_forwarded FROM relay_usage WHERE account_id=?1 AND period=?2",
        rusqlite::params![acc, signaling::admin::current_period()],
        |r| r.get(0),
    ).unwrap();
    drop(conn);
    assert_eq!(old, 116, "旧周期行保持滚动前的尾巴");
    assert_eq!(new, 216, "新周期从零起算,只含滚动后的字节");
    // 余量按新周期重认后随记账继续扣减:全额 - 滚动后本测第二包 216
    assert_eq!(
        state.relay.get().unwrap().quota.lock().unwrap().accounts.get(&acc).unwrap().remaining,
        signaling::admin::default_quota_bytes() as u64 - 216,
    );
}

#[tokio::test]
async fn patrol_hot_adjusts_rate_and_kills_disabled() {
    let (_addr, state) = signaling::spawn_test_server().await;
    let _relay_addr = spawn_relay_on(&state).await;
    let acc = prep_account(&state, "p@x.y");
    let (sid, tag_a, _tag_b) = insert_session(&state, 7, &acc, 1024);

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Message>(8);
    state.presence.insert("dev-a", PeerHandle { account_id: acc.clone(), tx });

    // 热调:配置改 5000 → 巡检后立即生效(逻辑层验证,不做绝对时长断言)
    signaling::admin::relay_set_limit(&state.db.lock().unwrap(), "p@x.y", 5000).unwrap();
    sweep_refresh(&state).unwrap();
    assert_eq!(
        state.relay.get().unwrap().registry.lock().unwrap().get_mut(sid).unwrap().rate_kbps,
        5000,
        "巡检须把配置限速热调进会话"
    );

    // 停用:巡检杀会话 + presence 通告 alive:false(裁定 C:60s 周期内生效,
    // 测试按裁定直接调巡检函数一次等效)
    signaling::admin::relay_set_enabled(&state.db.lock().unwrap(), "p@x.y", false).unwrap();
    sweep_refresh(&state).unwrap();
    assert!(state.relay.get().unwrap().registry.lock().unwrap().lookup(&tag_a).is_none());
    let msg = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.unwrap().unwrap();
    let Message::Binary(bin) = msg else { panic!("expected binary frame") };
    match WsServerFrame::decode(bin.as_ref()).unwrap().frame {
        Some(ws_server_frame::Frame::RelaySession(rs)) => {
            assert_eq!(rs.sid, sid);
            assert!(!rs.alive);
        }
        other => panic!("expected relay session frame, got {other:?}"),
    }

    // 巡检对未启动中继的 state 应为无害 no-op(冷启动先巡检不炸)
    let (_addr2, state2) = signaling::spawn_test_server().await;
    sweep_refresh(&state2).unwrap();
}

#[tokio::test]
async fn session_rate_gate_drops_and_recovers() {
    let (_addr, state) = signaling::spawn_test_server().await;
    let relay_addr = spawn_relay_on(&state).await;
    let acc = prep_account(&state, "rate@x.y");
    let a = bind_peer(&relay_addr).await;
    let b = bind_peer(&relay_addr).await;
    // 1 kbps = 125 B/s,满桶容量 125B;预设槽位保证首个线上包就是测试首包
    //(握手包会先消耗桶余额,破坏满桶起点)
    let (_sid, tag_a, _tag_b) = insert_session_at(
        &state,
        8,
        &acc,
        1,
        [Some(a.local_addr().unwrap()), Some(b.local_addr().unwrap())],
    );

    // 第一包过闸(116 ≤ 125)
    send(&a, &tag_a, &[0u8; 100]).await;
    assert_eq!(recv(&b).await.len(), 100);
    // 第二包超限:丢弃(QUIC 重传自会降速),不转发
    send(&a, &tag_a, &[1u8; 100]).await;
    let mut buf = [0u8; 1500];
    let dropped = tokio::time::timeout(Duration::from_millis(300), b.recv(&mut buf))
        .await
        .is_err();
    assert!(dropped, "超限数据报不应被转发");

    // 逻辑层可恢复性:约 1s 后桶回血,同量级报文再过(只验"可恢复",不钉绝对时长)
    tokio::time::sleep(Duration::from_millis(1100)).await;
    send(&a, &tag_a, &[2u8; 50]).await;
    assert_eq!(recv(&b).await, [2u8; 50]);
}

// Review Focus #4:全局上限独立生效——多会话共享同一总闸,与每会话限速叠加;
// 总闸 rate=0 为"不限",跳过全局闸(数据面对 0 的旁路分支)。
#[tokio::test]
async fn global_bucket_gate_shares_and_recovers() {
    let (_addr, state) = signaling::spawn_test_server().await;
    let relay_addr = spawn_relay_on(&state).await;
    let acc = prep_account(&state, "global@x.y");
    let a1 = bind_peer(&relay_addr).await;
    let b1 = bind_peer(&relay_addr).await;
    let a2 = bind_peer(&relay_addr).await;
    let b2 = bind_peer(&relay_addr).await;
    // 两个会话(rate=0 跳过会话桶,排除干扰),热调总闸到 1kbps(125B 满桶)
    let (_s1, t1a, _t1b) = insert_session_at(
        &state,
        9,
        &acc,
        0,
        [Some(a1.local_addr().unwrap()), Some(b1.local_addr().unwrap())],
    );
    let (_s2, t2a, _t2b) = insert_session_at(
        &state,
        10,
        &acc,
        0,
        [Some(a2.local_addr().unwrap()), Some(b2.local_addr().unwrap())],
    );
    state
        .relay
        .get()
        .unwrap()
        .global_bucket
        .lock()
        .unwrap()
        .set_rate(1, std::time::Instant::now());

    // 会话 1 首包过闸(116 ≤ 125),余额剩 ~9B
    send(&a1, &t1a, &[0u8; 100]).await;
    assert_eq!(recv(&b1).await.len(), 100);
    // 会话 2 共享同一总闸:余额不足即被全局闸丢弃(会话桶 rate=0 未参与)
    send(&a2, &t2a, &[1u8; 100]).await;
    let mut buf = [0u8; 1500];
    let dropped = tokio::time::timeout(Duration::from_millis(300), b2.recv(&mut buf))
        .await
        .is_err();
    assert!(dropped, "全局总闸耗尽时其他会话的报文也应被丢弃");

    // 总闸 rate=0(不限):即便令牌余额恰为 0 也不拦(旁路分支钉住)
    state
        .relay
        .get()
        .unwrap()
        .global_bucket
        .lock()
        .unwrap()
        .set_rate(0, std::time::Instant::now());
    send(&a2, &t2a, &[2u8; 50]).await;
    assert_eq!(recv(&b2).await, [2u8; 50]);

    // 恢复:切回 1kbps 时桶已真耗尽(set_rate(0) 清零),回血约 1s 补 125B 后
    // 同量级报文再过(只验"可恢复",不钉绝对时长)
    state
        .relay
        .get()
        .unwrap()
        .global_bucket
        .lock()
        .unwrap()
        .set_rate(1, std::time::Instant::now());
    tokio::time::sleep(Duration::from_millis(1100)).await;
    send(&a2, &t2a, &[3u8; 50]).await;
    assert_eq!(recv(&b2).await, [3u8; 50]);
}

// ===== T4:WS 分配面(RelayAllocate 门禁 + 双帧)=====

/// tokio-tungstenite 客户端流别名(签名简写)。
type Ws = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

/// REST 注册设备,取 (jwt, device_id)。沿 tests/ws.rs 同款:常量字节多半不是
/// 合法 Ed25519 曲线点,须用 seed 派生真实密钥对保证 pubkey 可解压。
async fn register(
    state: &std::sync::Arc<signaling::AppState>,
    addr: &std::net::SocketAddr,
    email: &str,
    seed: &[u8; 32],
    name: &str,
) -> (String, String) {
    signaling::admin::user_add(&state.db.lock().unwrap(), email).unwrap();
    let vk = ed25519_dalek::SigningKey::from_bytes(seed)
        .verifying_key()
        .to_bytes();
    let client = reqwest::Client::new();
    let code: serde_json::Value = client
        .post(format!("http://{addr}/auth/request-code"))
        .json(&serde_json::json!({"email": email}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let v: serde_json::Value = client
        .post(format!("http://{addr}/auth/verify"))
        .json(&serde_json::json!({
            "email": email, "code": code["code"].as_str().unwrap(),
            "pubkey": hex::encode(vk), "name": name, "platform": "test"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (
        v["jwt"].as_str().unwrap().into(),
        v["device_id"].as_str().unwrap().into(),
    )
}

async fn connect_ws(addr: &std::net::SocketAddr) -> Ws {
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    ws
}

async fn ws_send(ws: &mut Ws, frame: WsClientFrame) {
    ws.send(tokio_tungstenite::tungstenite::Message::Binary(frame.encode_to_vec()))
        .await
        .unwrap();
}

/// 收下一帧(跳过 Ping;Close/协议错误即 panic)。
async fn ws_recv(ws: &mut Ws) -> WsServerFrame {
    loop {
        match ws.next().await.unwrap().unwrap() {
            tokio_tungstenite::tungstenite::Message::Binary(b) => {
                return WsServerFrame::decode(b.as_ref()).unwrap()
            }
            tokio_tungstenite::tungstenite::Message::Ping(_) => continue,
            other => panic!("unexpected {other:?}"),
        }
    }
}

/// Hello 握手并回读服务端应答。
async fn hello(ws: &mut Ws, jwt: &str, device_id: &str) -> WsServerFrame {
    ws_send(
        ws,
        WsClientFrame {
            frame: Some(ws_client_frame::Frame::Hello(Hello {
                device_id: device_id.into(),
                jwt: jwt.into(),
            })),
        },
    )
    .await;
    ws_recv(ws).await
}

/// 发 RelayAllocate 申请。
async fn allocate(ws: &mut Ws, to_device: &str) {
    ws_send(
        ws,
        WsClientFrame {
            frame: Some(ws_client_frame::Frame::RelayAllocate(RelayAllocate {
                to_device: to_device.into(),
            })),
        },
    )
    .await;
}

/// 排掉对端上线 Presence 帧(双端同账号时申请方的下一帧恒为它,显式排掉钉帧序)。
async fn expect_peer_online(ws: &mut Ws) {
    let f = ws_recv(ws).await;
    // 先落盘实际帧(matches! 会按值吃掉 f.frame,消息里不能再借 f)
    let got = format!("{f:?}");
    assert!(
        matches!(f.frame, Some(ws_server_frame::Frame::Presence(p)) if p.online),
        "expected peer-online presence, got {got}"
    );
}

/// 断言下一帧是指定码的 Error(带实际帧的失败信息)。
async fn expect_error(ws: &mut Ws, code: u32, what: &str) {
    let f = ws_recv(ws).await;
    let got = format!("{f:?}");
    assert!(
        matches!(f.frame, Some(ws_server_frame::Frame::Error(e)) if e.code == code),
        "{what}: expected Error({code}), got {got}"
    );
}

#[tokio::test]
async fn ws_allocate_happy_both_frames_tags_distinct_rate_from_config() {
    let (addr, state) = signaling::spawn_test_server().await;
    let _relay_addr = spawn_relay_on(&state).await;
    let acc = prep_account(&state, "wsa@x.y");
    // 显式设非默认限速:证 rate 取自账号配置而非写死
    signaling::admin::relay_set_limit(&state.db.lock().unwrap(), "wsa@x.y", 2048).unwrap();

    let (jwt_a, dev_a) = register(&state, &addr, "wsa@x.y", &[21u8; 32], "A").await;
    let (jwt_b, dev_b) = register(&state, &addr, "wsa@x.y", &[22u8; 32], "B").await;
    let mut ws_a = connect_ws(&addr).await;
    let mut ws_b = connect_ws(&addr).await;
    hello(&mut ws_a, &jwt_a, &dev_a).await;
    hello(&mut ws_b, &jwt_b, &dev_b).await;
    expect_peer_online(&mut ws_a).await;

    allocate(&mut ws_a, &dev_b).await;

    // 请求方收 RelayAllocated(请求方槽 tag)
    let f = ws_recv(&mut ws_a).await;
    let (sid, tag_req, expires_at) = match f.frame {
        Some(ws_server_frame::Frame::RelayAllocated(ra)) => {
            assert_eq!(
                ra.relay_addr, "127.0.0.1:0",
                "relay_addr 须为 RelayState.pub_addr"
            );
            assert_eq!(ra.rate_kbps, 2048, "rate_kbps 须为账号配置限速");
            let expect = signaling::db::now() + 86_400;
            assert!(
                (ra.expires_at - expect).abs() <= 5,
                "expires_at 须为 now+86400, got {}",
                ra.expires_at
            );
            assert_eq!(ra.tag.len(), 16, "tag 须为 16 字节槽签");
            (ra.sid, ra.tag, ra.expires_at)
        }
        other => panic!("expected relay allocated, got {other:?}"),
    };

    // 对端收 RelaySession{alive:true, 对端槽 tag}
    let f = ws_recv(&mut ws_b).await;
    let tag_peer = match f.frame {
        Some(ws_server_frame::Frame::RelaySession(rs)) => {
            assert_eq!(rs.sid, sid, "双帧须同会话号");
            assert!(rs.alive);
            assert_eq!(rs.relay_addr, "127.0.0.1:0");
            assert_eq!(rs.tag.len(), 16);
            rs.tag
        }
        other => panic!("expected relay session, got {other:?}"),
    };

    // 两槽 tag 互异,且按槽位登记进注册表(请求方=槽 0,对端=槽 1)
    assert_ne!(tag_req, tag_peer, "双向槽签必不同");
    let relay = state.relay.get().unwrap();
    let mut reg = relay.registry.lock().unwrap();
    assert_eq!(reg.lookup(&tag_req), Some((sid, 0)), "请求方持槽 0");
    assert_eq!(reg.lookup(&tag_peer), Some((sid, 1)), "对端持槽 1");
    let s = reg.get_mut(sid).unwrap();
    assert_eq!(s.account_id, acc);
    assert_eq!(s.rate_kbps, 2048, "会话限速 = 账号配置");
    assert_eq!(s.ticket.expires_at, expires_at, "在册票据过期时刻 = 下发值");
    assert_eq!(reg.count_for_account(&acc), 1);
}

#[tokio::test]
async fn ws_allocate_refused_when_disabled_or_account_gone() {
    let (addr, state) = signaling::spawn_test_server().await;
    let _relay_addr = spawn_relay_on(&state).await;
    // 预置但不开通(user_add 预建行 enabled=0)
    signaling::admin::user_add(&state.db.lock().unwrap(), "off@x.y").unwrap();
    let (jwt_a, dev_a) = register(&state, &addr, "off@x.y", &[23u8; 32], "A").await;
    let (jwt_b, dev_b) = register(&state, &addr, "off@x.y", &[24u8; 32], "B").await;
    let mut ws_a = connect_ws(&addr).await;
    let mut ws_b = connect_ws(&addr).await;
    hello(&mut ws_a, &jwt_a, &dev_a).await;
    hello(&mut ws_b, &jwt_b, &dev_b).await;
    expect_peer_online(&mut ws_a).await;

    // 未开通:即使对端在线也拒 4
    allocate(&mut ws_a, &dev_b).await;
    expect_error(&mut ws_a, 4, "未开通账号").await;

    // 会话中途删号(裁定 B:已连 WS 不踢):再申请仍拒 4——账号不存在归并同码
    assert!(signaling::admin::user_remove(&state.db.lock().unwrap(), "off@x.y").unwrap());
    allocate(&mut ws_a, &dev_b).await;
    expect_error(&mut ws_a, 4, "账号不存在").await;
}

#[tokio::test]
async fn ws_allocate_refused_when_quota_exhausted() {
    let (addr, state) = signaling::spawn_test_server().await;
    let _relay_addr = spawn_relay_on(&state).await;
    let acc = prep_account(&state, "q5@x.y");
    let (jwt_a, dev_a) = register(&state, &addr, "q5@x.y", &[25u8; 32], "A").await;
    let (jwt_b, dev_b) = register(&state, &addr, "q5@x.y", &[26u8; 32], "B").await;
    let mut ws_a = connect_ws(&addr).await;
    let mut ws_b = connect_ws(&addr).await;
    hello(&mut ws_a, &jwt_a, &dev_a).await;
    hello(&mut ws_b, &jwt_b, &dev_b).await;
    expect_peer_online(&mut ws_a).await;

    // 当期用量打满配额(1 GiB = relay_set_quota 最小非零粒度):余量实时查库为 0
    signaling::admin::relay_set_quota(&state.db.lock().unwrap(), "q5@x.y", 1).unwrap();
    state.db.lock().unwrap().execute(
        "INSERT OR REPLACE INTO relay_usage(account_id, period, bytes_forwarded) VALUES(?1, ?2, ?3)",
        rusqlite::params![acc, signaling::admin::current_period(), 1_073_741_824i64],
    ).unwrap();

    allocate(&mut ws_a, &dev_b).await;
    expect_error(&mut ws_a, 5, "余量为 0").await;
}

#[tokio::test]
async fn ws_allocate_refused_when_concurrency_capped() {
    let (addr, state) = signaling::spawn_test_server().await;
    let _relay_addr = spawn_relay_on(&state).await;
    let acc = prep_account(&state, "c6@x.y");
    let (jwt_a, dev_a) = register(&state, &addr, "c6@x.y", &[27u8; 32], "A").await;
    let (jwt_b, dev_b) = register(&state, &addr, "c6@x.y", &[28u8; 32], "B").await;
    let mut ws_a = connect_ws(&addr).await;
    let mut ws_b = connect_ws(&addr).await;
    hello(&mut ws_a, &jwt_a, &dev_a).await;
    hello(&mut ws_b, &jwt_b, &dev_b).await;
    expect_peer_online(&mut ws_a).await;

    // 该账号已有两个在册会话(直插注册表,等价已满额的历史分配)
    insert_session(&state, 41, &acc, 1024);
    insert_session(&state, 42, &acc, 1024);

    allocate(&mut ws_a, &dev_b).await;
    expect_error(&mut ws_a, 6, "并发满 2").await;
}

#[tokio::test]
async fn ws_allocate_refused_when_peer_offline_or_cross_account() {
    let (addr, state) = signaling::spawn_test_server().await;
    let _relay_addr = spawn_relay_on(&state).await;
    let _acc = prep_account(&state, "p2@x.y");
    let (jwt_a, dev_a) = register(&state, &addr, "p2@x.y", &[29u8; 32], "A").await;
    let (_jwt_x, dev_x) = register(&state, &addr, "x2@y.z", &[30u8; 32], "X").await;
    let mut ws_a = connect_ws(&addr).await;
    hello(&mut ws_a, &jwt_a, &dev_a).await;

    // 对端不在线(复用转发同款拒绝码 2)
    allocate(&mut ws_a, "ffffffffffffffffffffffffffffffff").await;
    expect_error(&mut ws_a, 2, "对端不在线").await;

    // 跨账号在线设备同样拒 2(门禁要求对端同账号)
    let mut ws_x = connect_ws(&addr).await;
    let f = hello(&mut ws_x, &_jwt_x, &dev_x).await;
    assert!(matches!(f.frame, Some(ws_server_frame::Frame::HelloOk(_))));

    allocate(&mut ws_a, &dev_x).await;
    expect_error(&mut ws_a, 2, "跨账号对端").await;
}

#[tokio::test]
async fn ws_allocated_session_killed_by_patrol_when_disabled() {
    let (addr, state) = signaling::spawn_test_server().await;
    let _relay_addr = spawn_relay_on(&state).await;
    let _acc = prep_account(&state, "kill@x.y");
    let (jwt_a, dev_a) = register(&state, &addr, "kill@x.y", &[31u8; 32], "A").await;
    let (jwt_b, dev_b) = register(&state, &addr, "kill@x.y", &[32u8; 32], "B").await;
    let mut ws_a = connect_ws(&addr).await;
    let mut ws_b = connect_ws(&addr).await;
    hello(&mut ws_a, &jwt_a, &dev_a).await;
    hello(&mut ws_b, &jwt_b, &dev_b).await;
    expect_peer_online(&mut ws_a).await;

    allocate(&mut ws_a, &dev_b).await;
    let f = ws_recv(&mut ws_a).await;
    let (sid, tag_req) = match f.frame {
        Some(ws_server_frame::Frame::RelayAllocated(ra)) => (ra.sid, ra.tag),
        other => panic!("expected relay allocated, got {other:?}"),
    };
    let f = ws_recv(&mut ws_b).await;
    assert!(
        matches!(f.frame, Some(ws_server_frame::Frame::RelaySession(ref rs)) if rs.alive),
        "分配时对端须收 alive:true, got {f:?}"
    );

    // disable 后巡检一次(测试注入:直调巡检函数,等效 60s 周期到点)
    signaling::admin::relay_set_enabled(&state.db.lock().unwrap(), "kill@x.y", false).unwrap();
    sweep_refresh(&state).unwrap();
    assert!(
        state
            .relay
            .get()
            .unwrap()
            .registry
            .lock()
            .unwrap()
            .lookup(&tag_req)
            .is_none(),
        "disable 后巡检须摘除 WS 分配的会话"
    );

    // 双端各收 alive:false 通告(账号广播)
    for (ws, who) in [(&mut ws_a, "A"), (&mut ws_b, "B")] {
        let f = ws_recv(ws).await;
        match f.frame {
            Some(ws_server_frame::Frame::RelaySession(rs)) => {
                assert_eq!(rs.sid, sid, "{who} 通告须携带被杀会话号");
                assert!(!rs.alive, "{who} 须收 alive:false");
            }
            other => panic!("{who}: expected relay session frame, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn ws_allocate_with_stuck_peer_still_answers_requester() {
    let (addr, state) = signaling::spawn_test_server().await;
    let _relay_addr = spawn_relay_on(&state).await;
    let acc = prep_account(&state, "stuck@x.y");
    let (jwt_a, dev_a) = register(&state, &addr, "stuck@x.y", &[33u8; 32], "A").await;

    // 旧对端以桩 handle 顶替:容量 1 且预灌满——推送必 try_send 失败
    //(等价最坏的"旧客户端不解新帧/写路径卡死"情形),钉住"推送尽力而为、
    // 不影响请求方应答"的契约;推送成功且真实送达的场景由 happy 测试覆盖。
    let (tx_stub, _rx_stub) = tokio::sync::mpsc::channel::<Message>(1);
    tx_stub.send(Message::Binary(vec![0u8].into())).await.unwrap();
    state.presence.insert("dev-old", PeerHandle { account_id: acc.clone(), tx: tx_stub });

    let mut ws_a = connect_ws(&addr).await;
    hello(&mut ws_a, &jwt_a, &dev_a).await;

    allocate(&mut ws_a, "dev-old").await;
    let f = ws_recv(&mut ws_a).await;
    let sid = match f.frame {
        Some(ws_server_frame::Frame::RelayAllocated(ra)) => ra.sid,
        other => panic!("expected relay allocated, got {other:?}"),
    };
    // 会话照常入册,推送失败不回滚分配
    assert!(state.relay.get().unwrap().registry.lock().unwrap().get_mut(sid).is_some());
}
