//! 中继数据面集成测试:真 UdpSocket 互发、NAT 重绑、畸形输入、记账落库、
//! 配额杀会话、月界滚动、巡检热调/停用杀。按计划钉死的数据报算法逐条钉住。

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::Message;
use mtdss_proto::ws::{ws_server_frame, WsServerFrame};
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
    eprintln!("DBG test relay ptr={:p}", relay as *const _);
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
