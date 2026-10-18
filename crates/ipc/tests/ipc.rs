//! mtdss-ipc 集成测试:七个用例覆盖套接字命名、NDJSON 编解码、错误回传、
//! 并发 id 路由、事件广播、快速失败与 1 MiB 大帧。

use interprocess::local_socket::tokio::{RecvHalf, SendHalf, Stream};
use interprocess::local_socket::{traits::tokio::Stream as _, GenericFilePath, ToFsName};
use mtdss_ipc::{serve, socket_path, Dispatcher, EventFrame, IpcClient, RpcError};
use serde_json::{json, Value};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::broadcast;

/// 测试用分发器:仅认识 "echo"(原样回显 params),其余方法一律 -32601。
struct Echo;

impl Dispatcher for Echo {
    fn dispatch(
        &self,
        method: String,
        params: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RpcError>> + Send>> {
        Box::pin(async move {
            if method == "echo" {
                Ok(params)
            } else {
                Err(RpcError {
                    code: -32601,
                    message: format!("method not found: {method}"),
                })
            }
        })
    }
}

/// 起一个 serve 实例(回显分发器),返回测试侧事件发送端。
fn spawn_server(home: &Path) -> broadcast::Sender<EventFrame> {
    let (tx, _) = broadcast::channel(64);
    let serve_tx = tx.clone();
    let home = home.to_path_buf();
    tokio::spawn(async move {
        if let Err(err) = serve(&home, Arc::new(Echo), serve_tx).await {
            eprintln!("serve failed: {err:#}");
        }
    });
    tx
}

/// 原始写一帧(一行)。
async fn raw_send(sh: &mut SendHalf, frame: &str) {
    sh.write_all(frame.as_bytes()).await.unwrap();
    sh.write_all(b"\n").await.unwrap();
    sh.flush().await.unwrap();
}

/// 原始读一行并解析为 JSON。
async fn raw_read_line(reader: &mut BufReader<RecvHalf>) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).await.expect("读行不应失败");
    serde_json::from_str(line.trim_end()).expect("帧应为合法 JSON")
}

/// 重试连接直到服务端就绪(上限 2s):serve 任务建立管道/套接字与测试首次
/// 连接之间存在调度竞态,首次 connect 失败是同步错误、不给 serve 让步机会。
async fn connect_retry(home: &Path) -> IpcClient {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match IpcClient::connect(home).await {
            Ok(c) => return c,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(e) => panic!("连接服务端失败: {e:#}"),
        }
    }
}

#[tokio::test]
async fn socket_path_distinct_per_home() {
    let h1 = tempfile::tempdir().unwrap();
    let h2 = tempfile::tempdir().unwrap();
    let p1a = socket_path(h1.path());
    let p1b = socket_path(h1.path());
    let p2 = socket_path(h2.path());
    assert_eq!(p1a, p1b, "同 home 的 socket 路径必须稳定");
    assert_ne!(p1a, p2, "不同 home 的 socket 路径必须不同");
    #[cfg(windows)]
    assert!(p1a.starts_with(r"\\.\pipe\mtdss-"), "Windows 命名管道前缀: {p1a}");
}

#[tokio::test]
async fn codec_roundtrip() {
    let home = tempfile::tempdir().unwrap();
    let etx = spawn_server(home.path());
    // 原始 socket 客户端:手工收发 NDJSON,断言线上四种帧的精确形状
    let sp = socket_path(home.path());
    let deadline = Instant::now() + Duration::from_secs(2);
    let stream = loop {
        let name = sp.as_str().to_fs_name::<GenericFilePath>().unwrap();
        match Stream::connect(name).await {
            Ok(s) => break s,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(e) => panic!("连接服务端失败: {e}"),
        }
    };
    let (rh, mut sh) = stream.split();
    let mut reader = BufReader::new(rh);

    // 1) 请求/响应:{"id":N,"result":...}
    raw_send(&mut sh, r#"{"jsonrpc":"2.0","id":7,"method":"echo","params":{"msg":"hi"}}"#).await;
    let resp = raw_read_line(&mut reader).await;
    let mut keys: Vec<&str> = resp.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["id", "result"], "响应帧应为 {{id,result}}: {resp}");
    assert_eq!(resp["id"], 7);
    assert_eq!(resp["result"]["msg"], "hi");

    // 2) 错误:{"id":N,"error":{code,message}}
    raw_send(&mut sh, r#"{"jsonrpc":"2.0","id":8,"method":"nope"}"#).await;
    let resp = raw_read_line(&mut reader).await;
    let mut keys: Vec<&str> = resp.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["error", "id"], "错误帧应为 {{id,error}}: {resp}");
    assert_eq!(resp["id"], 8);
    assert_eq!(resp["error"]["code"], -32601);
    assert!(resp["error"]["message"].as_str().unwrap().contains("nope"));

    // 3) 事件:通知帧 {"jsonrpc":"2.0","method":...,"params":...}(无 id)
    etx.send(EventFrame {
        method: "devices.changed".into(),
        params: json!({"n": 1}),
    })
    .unwrap();
    let evt = raw_read_line(&mut reader).await;
    let mut keys: Vec<&str> = evt.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["jsonrpc", "method", "params"],
        "事件帧应为通知帧 {{jsonrpc,method,params}}(无 id): {evt}"
    );
    assert_eq!(evt["jsonrpc"], "2.0");
    assert_eq!(evt["method"], "devices.changed");
    assert_eq!(evt["params"]["n"], 1);
}

#[tokio::test]
async fn unknown_method_returns_minus_32601() {
    let home = tempfile::tempdir().unwrap();
    let _tx = spawn_server(home.path());
    let client = connect_retry(home.path()).await;
    let err = client.call("nope", json!({})).await.unwrap_err();
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("nope"), "错误信息应含方法名: {}", err.message);
}

#[tokio::test]
async fn echo_call_and_concurrent_calls() {
    let home = tempfile::tempdir().unwrap();
    let _tx = spawn_server(home.path());
    let c1 = connect_retry(home.path()).await;
    let c2 = connect_retry(home.path()).await;

    // 单次 echo(同时确保服务端已完成对 c1 的 accept)
    let r = c1.call("echo", json!({"hello": "世界"})).await.unwrap();
    assert_eq!(r["hello"], "世界");
    // 确保 c2 的 accept 也已完成,再开始并发
    c2.call("echo", json!({"warmup": true})).await.unwrap();

    // 两个客户端 × 各 8 个并发 call,id 路由不得错串
    let f11 = c1.call("echo", json!({"tag": "c1-0"}));
    let f12 = c1.call("echo", json!({"tag": "c1-1"}));
    let f13 = c1.call("echo", json!({"tag": "c1-2"}));
    let f14 = c1.call("echo", json!({"tag": "c1-3"}));
    let f15 = c1.call("echo", json!({"tag": "c1-4"}));
    let f16 = c1.call("echo", json!({"tag": "c1-5"}));
    let f17 = c1.call("echo", json!({"tag": "c1-6"}));
    let f18 = c1.call("echo", json!({"tag": "c1-7"}));
    let f21 = c2.call("echo", json!({"tag": "c2-0"}));
    let f22 = c2.call("echo", json!({"tag": "c2-1"}));
    let f23 = c2.call("echo", json!({"tag": "c2-2"}));
    let f24 = c2.call("echo", json!({"tag": "c2-3"}));
    let f25 = c2.call("echo", json!({"tag": "c2-4"}));
    let f26 = c2.call("echo", json!({"tag": "c2-5"}));
    let f27 = c2.call("echo", json!({"tag": "c2-6"}));
    let f28 = c2.call("echo", json!({"tag": "c2-7"}));
    let (r11, r12, r13, r14, r15, r16, r17, r18, r21, r22, r23, r24, r25, r26, r27, r28) =
        tokio::join!(
            f11, f12, f13, f14, f15, f16, f17, f18, f21, f22, f23, f24, f25, f26, f27, f28
        );
    for (i, r) in [r11, r12, r13, r14, r15, r16, r17, r18]
        .into_iter()
        .enumerate()
    {
        assert_eq!(r.unwrap()["tag"], format!("c1-{i}"));
    }
    for (i, r) in [r21, r22, r23, r24, r25, r26, r27, r28]
        .into_iter()
        .enumerate()
    {
        assert_eq!(r.unwrap()["tag"], format!("c2-{i}"));
    }
}

#[tokio::test]
async fn event_broadcast_reaches_all_clients() {
    let home = tempfile::tempdir().unwrap();
    let tx = spawn_server(home.path());
    let mut clients = Vec::new();
    let mut rxs = Vec::new();
    for _ in 0..3 {
        let c = connect_retry(home.path()).await;
        rxs.push(c.events());
        // 先做一次往返调用:响应返回即证明服务端已完成 accept(事件订阅先于发送)
        c.call("echo", json!({})).await.unwrap();
        clients.push(c);
    }
    tx.send(EventFrame {
        method: "tick".into(),
        params: json!({"n": 42}),
    })
    .unwrap();
    for mut rx in rxs {
        let evt = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("等待事件超时")
            .expect("事件通道不应关闭");
        assert_eq!(evt.method, "tick");
        assert_eq!(evt.params["n"], 42);
    }
}

#[tokio::test]
async fn missing_socket_fails_fast() {
    let home = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let outer =
        tokio::time::timeout(Duration::from_secs(2), IpcClient::connect(home.path())).await;
    let elapsed = started.elapsed();
    let inner = outer.expect("connect 必须在 2s 内返回,不得挂起");
    assert!(inner.is_err(), "对不存在的 socket,connect 应返回 Err");
    assert!(elapsed < Duration::from_secs(2), "connect 耗时 {elapsed:?}");
}

#[tokio::test]
async fn large_params_roundtrip() {
    let home = tempfile::tempdir().unwrap();
    let _tx = spawn_server(home.path());
    let client = connect_retry(home.path()).await;
    let big = "x".repeat(1024 * 1024);
    let r = client.call("echo", json!({"blob": big})).await.unwrap();
    assert_eq!(r["blob"].as_str().unwrap(), big.as_str(), "1 MiB 参数应原样往返");
}
