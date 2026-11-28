//! 控制通道(ctrl)端到端:真实 QUIC 对上的请求-响应往返 + 版本不符路径。
//!
//! 自连样板与 dual_endpoint.rs 一致:accept 必须与 connect 并发轮询
//! (connect_pinned 要等服务端在确认流上写令牌,顺序 await 会死锁)。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mtdss_core::identity::DeviceIdentity;
use mtdss_core::transport::ctrl::{
    send_request, send_request_streaming, serve_conn, CtrlReply, CtrlStreamError, CtrlStreamOut,
    CtrlStreamOutcome, STREAM_CHUNK_CAP,
};
use mtdss_core::transport::endpoint as ep;
use mtdss_proto::ctrl::{
    ctrl_request, ctrl_response, CtrlError, CtrlRequest, CtrlResponse, DirEntry, FileReadRangeRequest,
    ListRequest, ListResponse,
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
            atime_ms: 0,
            ctime_ms: 0,
            is_dir: false,
            children: None,
            duration_secs: 0.0,
        },
        DirEntry {
            name: "b".into(),
            path: "/tmp/b".into(),
            size: 0,
            mtime_ms: 1_700_000_000_001,
            atime_ms: 0,
            ctime_ms: 0,
            is_dir: true,
            children: None,
            duration_secs: 0.0,
        },
        DirEntry {
            name: "c.txt".into(),
            path: "/tmp/c.txt".into(),
            size: 34,
            mtime_ms: 1_700_000_000_002,
            atime_ms: 0,
            ctime_ms: 0,
            is_dir: false,
            children: None,
            duration_secs: 0.0,
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
            CtrlReply::Frame(CtrlResponse {
                version: CTRL_VERSION,
                ok: true,
                body: Some(ctrl_response::Body::List(ListResponse {
                    entries: fixed_entries(),
                    truncated: false,
                })),
            })
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
        CtrlReply::Frame(CtrlResponse {
            version: 0,
            ok: true,
            body: None,
        })
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
        CtrlReply::Frame(CtrlResponse {
            version: CTRL_VERSION,
            ok: !bad,
            body: bad.then(|| {
                ctrl_response::Body::Error(CtrlError {
                    code: "INVALID_PATH".into(),
                    message: "unsupported request body".into(),
                })
            }),
        })
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

// ---------- V4-1 Task 2:ctrl 流式多帧响应(预览数据面基座) ----------

/// 流式 echo 的应答半边:payload 逐块送入有界通道(生产端;拆帧与帧序归
/// 响应端泵,测试只钉消费面)。
fn stream_out(payload: Vec<Vec<u8>>) -> CtrlStreamOut {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tokio::spawn(async move {
        for p in payload {
            if tx.send(Ok(p)).await.is_err() {
                break;
            }
        }
    });
    CtrlStreamOut { chunks: rx }
}

/// file_read_range 请求(路径/偏移由调用方指定)。
fn read_range_req(path: &str, offset: u64, len: u32) -> CtrlRequest {
    CtrlRequest {
        version: CTRL_VERSION,
        body: Some(ctrl_request::Body::FileReadRange(FileReadRangeRequest {
            device_id: "self".into(),
            path: path.into(),
            mtime: 0,
            size: 0,
            offset,
            len,
        })),
    }
}

/// 流式 echo 往返(真实 QUIC 对,回环双端):发起方收齐全部块且序不乱、
/// 拼接保真、DataEnd 收尾;超过单帧帽的大块被响应端泵拆成多帧(帧计数
/// 钉 STREAM_CHUNK_CAP 拆分面)。
#[tokio::test]
async fn streaming_echo_roundtrip_over_quic() {
    let (client, server) = connected_pair().await;

    // 大块 = 帽的 2 倍 + 零头 → 泵必拆 3 帧;全量 = head(1) + big(3) +
    // tail(1) = 5 个 chunk 帧
    let big = vec![7u8; STREAM_CHUNK_CAP * 2 + 123];
    let payload = vec![b"head".to_vec(), big.clone(), b"tail".to_vec()];
    let want: Vec<u8> = payload.iter().flatten().copied().collect();
    tokio::spawn(serve_conn(server, move |req| {
        let payload = payload.clone();
        async move {
            match req.body {
                Some(ctrl_request::Body::FileReadRange(_)) => {
                    CtrlReply::Stream(stream_out(payload))
                }
                _ => CtrlReply::Frame(CtrlResponse {
                    version: CTRL_VERSION,
                    ok: false,
                    body: Some(ctrl_response::Body::Error(CtrlError {
                        code: "INVALID_PATH".into(),
                        message: "unsupported".into(),
                    })),
                }),
            }
        }
    }));

    let outcome = send_request_streaming(&client, &read_range_req("/big.bin", 0, 5), Duration::from_secs(5))
        .await
        .unwrap();
    let mut stream = match outcome {
        CtrlStreamOutcome::Stream(s) => s,
        CtrlStreamOutcome::Frame(r) => panic!("expected stream, got frame {r:?}"),
    };
    let sid = stream.stream_id();
    assert!(sid >= 1, "stream_id 必须是登记簿分配的正值: {sid}");

    let mut count = 0u32;
    let mut got = Vec::new();
    while let Some(item) = stream.next_chunk().await {
        let c = item.expect("echo 全程无中途错");
        got.extend_from_slice(&c);
        count += 1;
    }
    // 序不乱 + 内容保真
    assert_eq!(got, want);
    assert_eq!(count, 5, "head+大块拆 3+tail = 恰 5 个 chunk 帧");
    // DataEnd 后 next_chunk 恒 None
    assert!(stream.next_chunk().await.is_none());
}

/// 同一连接上两路并发流式响应 + 中途插一笔普通 list 请求:三面各自保真
/// ——stream_id 不撞号、数据不串台、普通请求-响应路径逐字节不受影响。
#[tokio::test]
async fn concurrent_streams_and_plain_request_interleave() {
    let (client, server) = connected_pair().await;

    tokio::spawn(serve_conn(server, move |req| {
        async move {
            match req.body {
                Some(ctrl_request::Body::FileReadRange(r)) => {
                    let p = if r.path == "/a" { b"AAAA".to_vec() } else { b"BBB".to_vec() };
                    CtrlReply::Stream(stream_out(vec![p.clone(), p]))
                }
                Some(ctrl_request::Body::List(_)) => CtrlReply::Frame(CtrlResponse {
                    version: CTRL_VERSION,
                    ok: true,
                    body: Some(ctrl_response::Body::List(ListResponse {
                        entries: fixed_entries(),
                        truncated: false,
                    })),
                }),
                _ => CtrlReply::Frame(CtrlResponse {
                    version: CTRL_VERSION,
                    ok: false,
                    body: Some(ctrl_response::Body::Error(CtrlError {
                        code: "INVALID_PATH".into(),
                        message: "unsupported".into(),
                    })),
                }),
            }
        }
    }));

    let t = Duration::from_secs(5);
    let req_a = read_range_req("/a", 0, 1);
    let req_b = read_range_req("/b", 0, 1);
    let (ra, rb) = tokio::join!(
        send_request_streaming(&client, &req_a, t),
        send_request_streaming(&client, &req_b, t),
    );
    let (mut sa, mut sb) = match (ra.unwrap(), rb.unwrap()) {
        (CtrlStreamOutcome::Stream(a), CtrlStreamOutcome::Stream(b)) => (a, b),
        _ => panic!("两路都必须进入流式"),
    };
    assert_ne!(sa.stream_id(), sb.stream_id(), "并发流 id 不得撞号");

    // 各收一块后中途插普通请求
    assert_eq!(sa.next_chunk().await.unwrap().unwrap(), b"AAAA");
    assert_eq!(sb.next_chunk().await.unwrap().unwrap(), b"BBB");
    let resp = send_request(&client, &list_req("/tmp"), t).await.unwrap();
    assert!(resp.ok);

    // 两路流各自收尾,数据互不串台(第二块同前缀,DataEnd 终止)
    assert_eq!(sa.next_chunk().await.unwrap().unwrap(), b"AAAA");
    assert!(sa.next_chunk().await.is_none());
    assert_eq!(sb.next_chunk().await.unwrap().unwrap(), b"BBB");
    assert!(sb.next_chunk().await.is_none());

    // 普通请求-响应保真(流式在途不扰既有 oneof 面)
    match resp.body {
        Some(ctrl_response::Body::List(l)) => assert_eq!(l.entries, fixed_entries()),
        other => panic!("expected list response, got {other:?}"),
    }
}

/// 流中生产者失败:响应端泵改写 CtrlError 帧收尾(不再有 DataEnd),发起
/// 方先收到在场块、随后以 code/message 显形,流即刻终止。
#[tokio::test]
async fn stream_producer_error_surfaces_as_ctrl_error() {
    let (client, server) = connected_pair().await;

    tokio::spawn(serve_conn(server, |_req| async move {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tokio::spawn(async move {
            tx.send(Ok(b"part1".to_vec())).await.unwrap();
            tx.send(Err(CtrlStreamError {
                code: "INTERNAL".into(),
                message: "盘读失败".into(),
            }))
            .await
            .unwrap();
        });
        CtrlReply::Stream(CtrlStreamOut { chunks: rx })
    }));

    let outcome = send_request_streaming(&client, &read_range_req("/p", 0, 1), Duration::from_secs(5))
        .await
        .unwrap();
    let mut s = match outcome {
        CtrlStreamOutcome::Stream(s) => s,
        CtrlStreamOutcome::Frame(r) => panic!("expected stream, got frame {r:?}"),
    };
    assert_eq!(s.next_chunk().await.unwrap().unwrap(), b"part1");
    let err = s.next_chunk().await.unwrap().expect_err("生产者失败必须显形");
    assert_eq!(err.code, "INTERNAL");
    assert_eq!(err.message, "盘读失败");
    assert!(s.next_chunk().await.is_none(), "错误后流即终止");
}

/// mux 直连(标签模式)上的流式往返:发起端 [`send_request_streaming_tagged`]
/// 先写 TAG_CTRL 一字节,响应端按 mux 分派读标签后交 [`serve_stream`] 泵
/// ——T3 池内 mux 直连的预览数据面将走此舞步(裸路径不写标签,帧序同形)。
#[tokio::test]
async fn streaming_roundtrip_over_tagged_mux_stream() {
    use mtdss_core::transport::ctrl::{
        send_request_streaming_tagged, serve_stream, StreamRegistry,
    };
    use mtdss_core::transport::mux::TAG_CTRL;

    let (client, server) = connected_pair().await;

    // 响应端按 daemon mux serve 形态:accept_bi → 读标签字节 → serve_stream
    tokio::spawn(async move {
        loop {
            let Ok((tx, mut rx)) = server.accept_bi().await else {
                return;
            };
            let mut tag = [0u8; 1];
            if rx.read_exact(&mut tag).await.is_err() {
                return;
            }
            assert_eq!(tag[0], TAG_CTRL, "标签模式流必须以 TAG_CTRL 起头");
            let reg = Arc::new(StreamRegistry::new());
            tokio::spawn(serve_stream(tx, rx, reg, |req| async move {
                match req.body {
                    Some(ctrl_request::Body::FileReadRange(_)) => {
                        CtrlReply::Stream(stream_out(vec![b"m1".to_vec(), b"m2".to_vec()]))
                    }
                    _ => CtrlReply::Frame(CtrlResponse {
                        version: CTRL_VERSION,
                        ok: false,
                        body: Some(ctrl_response::Body::Error(CtrlError {
                            code: "INVALID_PATH".into(),
                            message: "unsupported".into(),
                        })),
                    }),
                }
            }));
        }
    });

    let outcome = send_request_streaming_tagged(
        &client,
        &read_range_req("/mux.bin", 0, 1),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    let mut s = match outcome {
        CtrlStreamOutcome::Stream(s) => s,
        CtrlStreamOutcome::Frame(r) => panic!("expected stream, got frame {r:?}"),
    };
    assert_eq!(s.next_chunk().await.unwrap().unwrap(), b"m1");
    assert_eq!(s.next_chunk().await.unwrap().unwrap(), b"m2");
    assert!(s.next_chunk().await.is_none());
}

/// 旧端安全(未知请求变体):手造线上字节——version 场 + 未知 oneof 标签
/// (field 30,本版 CtrlRequest 不识;旧端 proto 同样不识)→ prost 解成
/// body=None → 响应端按既有「缺 body」臂回 CtrlError,不崩;连接存活,
/// 后续普通请求照常。
#[tokio::test]
async fn unknown_request_variant_gets_internal_error_and_conn_survives() {
    use prost::Message as _;
    // quinn 流自带 write_all/read_exact 固有方法,无需 tokio ext trait

    let (client, server) = connected_pair().await;

    tokio::spawn(serve_conn(server, |req| async move {
        match req.body {
            // 未知变体在旧端解成 body=None → 缺 body INTERNAL(既有契约,
            // T1 proto 注释钉死;新端 handler 分发臂同码)
            None => CtrlReply::Frame(CtrlResponse {
                version: CTRL_VERSION,
                ok: false,
                body: Some(ctrl_response::Body::Error(CtrlError {
                    code: "INTERNAL".into(),
                    message: "ctrl 请求缺 body".into(),
                })),
            }),
            // 普通请求照常(连接存活面)
            Some(ctrl_request::Body::List(_)) => CtrlReply::Frame(CtrlResponse {
                version: CTRL_VERSION,
                ok: true,
                body: Some(ctrl_response::Body::List(ListResponse {
                    entries: Vec::new(),
                    truncated: false,
                })),
            }),
            _ => CtrlReply::Frame(CtrlResponse {
                version: CTRL_VERSION,
                ok: false,
                body: Some(ctrl_response::Body::Error(CtrlError {
                    code: "INVALID_PATH".into(),
                    message: "unsupported".into(),
                })),
            }),
        }
    }));

    // 手造帧体:field1 varint(version=2)+ field30 长度限定未知变体载荷。
    // (30<<3)|2 = 242 → varint 标签 0xF2 0x01;载荷 2 字节 0xAA 0xBB。
    assert_eq!(CTRL_VERSION, 2, "字节字面量按 CTRL_VERSION=2 手工编码");
    let raw: &[u8] = &[0x08, 0x02, 0xF2, 0x01, 0x02, 0xAA, 0xBB];

    // 裸流直写(绕过 send_request 的类型面,模拟更新版发起端)
    let (mut tx, mut rx) = client.open_bi().await.unwrap();
    tx.write_all(&(raw.len() as u32).to_le_bytes()).await.unwrap();
    tx.write_all(raw).await.unwrap();
    let _ = tx.finish();
    let mut len_buf = [0u8; 4];
    rx.read_exact(&mut len_buf).await.unwrap();
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    rx.read_exact(&mut buf).await.unwrap();
    let resp = CtrlResponse::decode(buf.as_slice()).unwrap();

    // 未知变体 → CtrlError 回执,不崩
    assert!(!resp.ok);
    match resp.body {
        Some(ctrl_response::Body::Error(e)) => {
            assert_eq!(e.code, "INTERNAL");
            assert_eq!(e.message, "ctrl 请求缺 body");
        }
        other => panic!("expected internal error, got {other:?}"),
    }

    // 连接存活:同连接后续普通请求照常往返
    let resp = send_request(&client, &list_req("/tmp"), Duration::from_secs(5))
        .await
        .unwrap();
    assert!(resp.ok);
}

// ---------- V4-1 T3 补遗(T2 Minor 遗留):登记簿满帽拒绝帧与中途取消回收 ----------

/// 持槽流式应答的 handler 生产端:小间隔持续投递(通道有界;泵写入侧被
/// QUIC 流控/STOP_SENDING 收口后 send 败自收)。永不主动关通道——槽的
/// 生命周期完全由消费端行为驱动(取消才回收)。
fn held_stream_out() -> CtrlStreamOut {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(5)).await;
            if tx.send(Ok(vec![0u8; 1024])).await.is_err() {
                break;
            }
        }
    });
    CtrlStreamOut { chunks: rx }
}

/// 流帽满拒绝帧(T2 Minor 遗留①):同连接在途流登记簿占满
/// ([`STREAM_CONCURRENT_CAP`] 路,生产端持续投递、消费端不读 = 泵滞留
/// 写侧,守卫长持)后,第 N+1 路流式请求得**单帧 CtrlError**(INTERNAL,
/// 占满文案,无 DataBegin),且连接存活(后续普通请求照常)。帽满错误码
/// 沿用 INTERNAL——发起端不做依赖码区分的重试(评审转达⑥)。
#[tokio::test]
async fn stream_cap_full_rejects_with_single_error_frame() {
    use mtdss_core::transport::ctrl::STREAM_CONCURRENT_CAP;

    let (client, server) = connected_pair().await;

    tokio::spawn(serve_conn(server, move |req| async move {
        match req.body {
            Some(ctrl_request::Body::FileReadRange(_)) => CtrlReply::Stream(held_stream_out()),
            _ => CtrlReply::Frame(CtrlResponse {
                version: CTRL_VERSION,
                ok: true,
                body: Some(ctrl_response::Body::List(ListResponse {
                    entries: Vec::new(),
                    truncated: false,
                })),
            }),
        }
    }));

    // 占满:帽内每路都拿到 DataBegin(登记成功的行为证明)。手柄持至用例
    // 尾——RecvStream 弃柄即 STOP_SENDING,泵下次写败自收、槽位即刻回收,
    // 填充环内先开槽可能已释放,帽满断言就变成与回收时延赛跑(机器相关
    // 竞速,评审 Important #1);持柄使泵滞留流控写侧,帽满恒确定。
    let t = Duration::from_secs(5);
    let mut held = Vec::new();
    for i in 0..STREAM_CONCURRENT_CAP {
        let outcome =
            send_request_streaming(&client, &read_range_req(&format!("/h{i}"), 0, 1), t)
                .await
                .unwrap();
        match outcome {
            CtrlStreamOutcome::Stream(s) => held.push(s),
            CtrlStreamOutcome::Frame(r) => panic!("第 {i} 路帽内必须进流式: {r:?}"),
        }
    }
    assert_eq!(held.len(), STREAM_CONCURRENT_CAP);

    // 第 N+1 路:单帧 CtrlError(INTERNAL 占满文案),无 DataBegin
    let outcome = send_request_streaming(&client, &read_range_req("/overflow", 0, 1), t)
        .await
        .unwrap();
    match outcome {
        CtrlStreamOutcome::Frame(resp) => {
            assert!(!resp.ok);
            match resp.body {
                Some(ctrl_response::Body::Error(e)) => {
                    assert_eq!(e.code, "INTERNAL", "帽满沿用 INTERNAL(依赖码不可区分)");
                    assert!(
                        e.message.contains("并发超限"),
                        "帽满文案携因: {e:?}"
                    );
                }
                other => panic!("expected error body, got {other:?}"),
            }
        }
        CtrlStreamOutcome::Stream(_) => panic!("帽满必须单帧拒,不得进流式"),
    }

    // 连接存活:普通请求照常
    let resp = send_request(&client, &list_req("/tmp"), t).await.unwrap();
    assert!(resp.ok);
}

/// 中途取消回收(T2 Minor 遗留②):帽满后取消一路在途流(消费端弃手柄
/// = STOP_SENDING,响应端泵写侧即败收流、登记 id 回收)→ 新流式请求在
/// 限期内成功(回收证明);回收恰一路——其余帽位仍持,再下一路依旧单帧
/// 拒(帽重新收口)。
#[tokio::test]
async fn mid_stream_cancel_recycles_registry_slot() {
    use mtdss_core::transport::ctrl::STREAM_CONCURRENT_CAP;

    let (client, server) = connected_pair().await;

    tokio::spawn(serve_conn(server, move |req| async move {
        match req.body {
            Some(ctrl_request::Body::FileReadRange(_)) => CtrlReply::Stream(held_stream_out()),
            _ => CtrlReply::Frame(CtrlResponse {
                version: CTRL_VERSION,
                ok: true,
                body: Some(ctrl_response::Body::List(ListResponse {
                    entries: Vec::new(),
                    truncated: false,
                })),
            }),
        }
    }));

    let t = Duration::from_secs(5);
    // 占满后弃首路(取消)
    let mut held = Vec::new();
    for i in 0..STREAM_CONCURRENT_CAP {
        let outcome =
            send_request_streaming(&client, &read_range_req(&format!("/c{i}"), 0, 1), t)
                .await
                .unwrap();
        held.push(match outcome {
            CtrlStreamOutcome::Stream(s) => s,
            CtrlStreamOutcome::Frame(r) => panic!("第 {i} 路帽内必须进流式: {r:?}"),
        });
    }
    drop(held.remove(0)); // 中途取消:STOP_SENDING → 泵写败 → 守卫回收

    // 回收证明:重试至一路成功(泵收尾需要一小拍,轮询不设脆墙钟)
    let mut recycled: Option<CtrlStreamOutcome> = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        match send_request_streaming(&client, &read_range_req("/recycled", 0, 1), t).await {
            Ok(o @ CtrlStreamOutcome::Stream(_)) => {
                recycled = Some(o);
                break;
            }
            Ok(CtrlStreamOutcome::Frame(_)) => {
                // 仍满:等一拍再试
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => panic!("回收轮询不得失败(连接须活): {e:#}"),
        }
    }
    assert!(recycled.is_some(), "取消后 5s 内必有流位回收(登记簿回收面)");

    // 帽重新收口:回收恰一路,再下一路仍单帧拒
    let outcome = send_request_streaming(&client, &read_range_req("/again", 0, 1), t)
        .await
        .unwrap();
    match outcome {
        CtrlStreamOutcome::Frame(resp) => {
            assert!(!resp.ok, "回收恰一路,帽仍满: {resp:?}");
        }
        CtrlStreamOutcome::Stream(_) => panic!("帽必须重新收口,不得再进流式"),
    }
}
