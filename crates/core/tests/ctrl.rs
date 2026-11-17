//! 控制通道(ctrl)端到端:真实 QUIC 对上的请求-响应往返 + 版本不符路径。
//!
//! 自连样板与 dual_endpoint.rs 一致:accept 必须与 connect 并发轮询
//! (connect_pinned 要等服务端在确认流上写令牌,顺序 await 会死锁)。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mtdss_core::identity::DeviceIdentity;
use mtdss_core::transport::ctrl::{send_request, serve_conn};
use mtdss_core::transport::endpoint as ep;
use mtdss_proto::ctrl::{
    ctrl_request, ctrl_response, CtrlError, CtrlRequest, CtrlResponse, DirEntry, ListRequest,
    ListResponse,
};
use mtdss_proto::CTRL_VERSION;

fn bind0() -> (std::net::UdpSocket, std::net::SocketAddr) {
    let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let a = s.local_addr().unwrap();
    (s, a)
}

/// 已认证的自连 QUIC 对:(客户端连接, 服务端连接)。
async fn connected_pair() -> (quinn::Connection, quinn::Connection) {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let allowed_a: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(a.device_id.clone(), a.public_key())].into_iter().collect();
    let allowed_b: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(b.device_id.clone(), b.public_key())].into_iter().collect();

    let (sa, _addr_a) = bind0();
    let (sb, addr_b) = bind0();
    let ep_a = ep::make_dual_endpoint(sa, &a, &allowed_b).unwrap();
    let ep_b = ep::make_dual_endpoint(sb, &b, &allowed_a).unwrap();

    let acc = {
        let ep_b = ep_b.clone();
        tokio::spawn(async move { ep::accept_pinned(&ep_b, &allowed_a).await.unwrap() })
    };
    let client = ep::connect_pinned(&ep_a, addr_b, &b.public_key(), &a).await.unwrap();
    let (server, dev) = acc.await.unwrap();
    assert_eq!(dev, a.device_id);
    (client, server)
}

/// list 请求(路径由调用方指定)。
fn list_req(path: &str) -> CtrlRequest {
    CtrlRequest {
        version: CTRL_VERSION,
        body: Some(ctrl_request::Body::List(ListRequest { path: path.into() })),
    }
}

/// echo handler 的 3 个固定 DirEntry(children 字段 = T5b 增量的消息包裹
/// 可选面,echo 夹具一律 None——计数语义由 daemon 侧单测钉)。
fn fixed_entries() -> Vec<DirEntry> {
    vec![
        DirEntry {
            name: "a.txt".into(),
            path: "/tmp/a.txt".into(),
            size: 12,
            mtime_ms: 1_700_000_000_000,
            is_dir: false,
            children: None,
        },
        DirEntry {
            name: "b".into(),
            path: "/tmp/b".into(),
            size: 0,
            mtime_ms: 1_700_000_000_001,
            is_dir: true,
            children: None,
        },
        DirEntry {
            name: "c.txt".into(),
            path: "/tmp/c.txt".into(),
            size: 34,
            mtime_ms: 1_700_000_000_002,
            is_dir: false,
            children: None,
        },
    ]
}

#[tokio::test]
async fn list_request_response_roundtrip_over_quic() {
    let (client, server) = connected_pair().await;

    // 响应端:echo handler —— list 请求回 3 个固定 DirEntry;同时记下收到的
    // 请求路径,供请求侧字段保真断言。
    let seen = Arc::new(Mutex::new(None::<String>));
    let seen_cl = seen.clone();
    tokio::spawn(serve_conn(server, move |req| {
        // 外层 Fn 闭包按流调用多次:每次进 async 前克隆自有 Arc 副本
        let seen_cl = seen_cl.clone();
        async move {
            if let Some(ctrl_request::Body::List(l)) = req.body {
                *seen_cl.lock().unwrap() = Some(l.path);
            }
            CtrlResponse {
                version: CTRL_VERSION,
                ok: true,
                body: Some(ctrl_response::Body::List(ListResponse {
                    entries: fixed_entries(),
                    truncated: false,
                })),
            }
        }
    }));

    let resp = send_request(&client, &list_req("/tmp"), Duration::from_secs(5))
        .await
        .unwrap();

    // 请求侧字段保真:服务端 handler 收到的路径逐字节一致
    assert_eq!(seen.lock().unwrap().as_deref(), Some("/tmp"));

    // 响应侧字段往返
    assert_eq!(resp.version, CTRL_VERSION);
    assert!(resp.ok);
    match resp.body {
        Some(ctrl_response::Body::List(l)) => {
            assert!(!l.truncated);
            assert_eq!(l.entries, fixed_entries());
        }
        other => panic!("expected list response, got {other:?}"),
    }
}

#[tokio::test]
async fn version_mismatch_rejected_on_send_side() {
    let (client, server) = connected_pair().await;

    // 手造 version=0 响应端:serve_conn 不改写 handler 的响应,原样回帧,
    // send 侧必须以 UNSUPPORTED_VERSION 语义报错。
    tokio::spawn(serve_conn(server, |_req| async {
        CtrlResponse {
            version: 0,
            ok: true,
            body: None,
        }
    }));

    let err = send_request(&client, &list_req("/tmp"), Duration::from_secs(5))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("UNSUPPORTED_VERSION"),
        "expected UNSUPPORTED_VERSION, got: {err}"
    );
}

/// 非 list/stat 的畸形请求体:handler 收不到匹配分支,回 CtrlError 语义
/// (ok=false)。此处只验证 ok/error 字段能往返(编解码层兜底)。
#[tokio::test]
async fn error_response_fields_roundtrip() {
    let (client, server) = connected_pair().await;

    tokio::spawn(serve_conn(server, |req| async move {
        let bad = !matches!(req.body, Some(ctrl_request::Body::List(_)));
        CtrlResponse {
            version: CTRL_VERSION,
            ok: !bad,
            body: bad.then(|| {
                ctrl_response::Body::Error(CtrlError {
                    code: "INVALID_PATH".into(),
                    message: "unsupported request body".into(),
                })
            }),
        }
    }));

    // version 符合但 body 为空 → handler 判 bad → CtrlError 回来
    let req = CtrlRequest { version: CTRL_VERSION, body: None };
    let resp = send_request(&client, &req, Duration::from_secs(5)).await.unwrap();
    assert!(!resp.ok);
    match resp.body {
        Some(ctrl_response::Body::Error(e)) => {
            assert_eq!(e.code, "INVALID_PATH");
            assert_eq!(e.message, "unsupported request body");
        }
        other => panic!("expected error response, got {other:?}"),
    }
}

// ---------- ctrl 流截断家族(V3-7 票① Step 1 复现锚) ----------
//
// 统一签名「EOF while parsing a value at line 1 column 0」的定性证据:ctrl
// 面帧协议是 prost(`[u32 len][prost bytes]`),流被截断(serve 端 accept 后
// 不开写直接关)出生的是**帧读取错误**,绝不是 serde EOF——该签名只能
// 出生在 HTTP 面 `resp.json()` 的空 body 解析(daemon account.rs,分类
// 重试在彼处修)。以下两例同时钉住 daemon ctrl 池分类缝的输入面:流级
// 截断(连接仍活)与连接级死亡(连接死透)在传输层就可分辨,池的
// Surface(验证缝重发恰一次)/EvictAndRetry(驱逐整程重试恰一次)分派
// 在此输入上已有界——本层无需修复,行为如例钉死。

/// 流级截断:serve 端把首条流的请求读完即弃流(不开写响应,响应侧 0
/// 字节 = 「空 body」)→ send_request 报**帧读取**错误(绝非 serde EOF
/// 文案);**连接仍活**:同连接第二笔请求照常成功(QUIC 流隔离,截断只
/// 停自己)。池侧对应输入 = conn_alive true → Surface 验证缝(另打新
/// 连接重发恰一次),已有界。
#[tokio::test]
async fn stream_truncated_empty_response_errors_but_conn_survives() {
    use mtdss_core::transport::ctrl::read_request_frame;

    let (client, server) = connected_pair().await;

    // serve:首条流读完请求即弃(零响应字节);此后每条流正常应答
    tokio::spawn(async move {
        let mut first = true;
        loop {
            let (mut tx, mut rx) = match server.accept_bi().await {
                Ok(v) => v,
                Err(_) => return,
            };
            if first {
                first = false;
                let _ = read_request_frame(&mut rx).await;
                drop(tx); // 不写响应:对端读侧见 0 字节即 FIN
                continue;
            }
            let resp = CtrlResponse {
                version: CTRL_VERSION,
                ok: true,
                body: Some(ctrl_response::Body::List(ListResponse {
                    entries: Vec::new(),
                    truncated: false,
                })),
            };
            tokio::spawn(async move {
                let _ = mtdss_core::transport::ctrl::write_response_frame(&mut tx, &resp).await;
            });
        }
    });

    let err = send_request(&client, &list_req("/tmp"), Duration::from_secs(5))
        .await
        .expect_err("流被关且零响应字节必须报错");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("read frame"),
        "帧层错误(长度前缀读失败): {msg}"
    );
    assert!(
        !msg.contains("EOF while parsing"),
        "ctrl 面 prost 帧不产 serde EOF 文案(签名出生地不在 QUIC 面): {msg}"
    );

    // 连接仍活且可复用:同连接第二笔请求成功(截断只停那条流自己)
    let resp = send_request(&client, &list_req("/tmp"), Duration::from_secs(5))
        .await
        .expect("流级截断后同连接第二笔必须成功(QUIC 流隔离)");
    assert!(resp.ok);
}

/// 连接级死亡:serve 端把首条流的请求读完后立即整连关闭 → send_request
/// 报错且**连接死透**(CONNECTION_CLOSE 到位后 open_bi 必败)。池侧对应
/// 输入 = conn_alive false → EvictAndRetry(驱逐 + 整程重试恰一次),已有界。
#[tokio::test]
async fn conn_closed_mid_request_errors_and_conn_is_dead() {
    use mtdss_core::transport::ctrl::read_request_frame;

    let (client, server) = connected_pair().await;

    tokio::spawn(async move {
        // 请求已在飞:读完即关整连,在飞流被截断
        if let Ok((tx, mut rx)) = server.accept_bi().await {
            let _ = read_request_frame(&mut rx).await;
            drop(tx);
            server.close(0u32.into(), b"");
        }
    });

    send_request(&client, &list_req("/tmp"), Duration::from_secs(5))
        .await
        .expect_err("连接中途关闭必须报错");

    // 连接死透:CONNECTION_CLOSE 到位后再开流必败(conn_alive 的输入面)
    client.closed().await;
    assert!(
        client.open_bi().await.is_err(),
        "连接级死亡后 open_bi 必须失败(conn_alive=false → 池驱逐臂)"
    );
}
