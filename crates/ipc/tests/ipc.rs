//! mtdss-ipc 集成测试:套接字命名、NDJSON 编解码、错误回传、并发 id 路由、
//! 事件广播、快速失败与 1 MiB 大帧,以及更新标记助手与连接计数(Task 12)。

use interprocess::local_socket::tokio::{RecvHalf, SendHalf, Stream};
use interprocess::local_socket::{traits::tokio::Stream as _, GenericFilePath, ToFsName};
use mtdss_ipc::{
    connected_clients, restart_ui_marker_path, serve, socket_path, update_in_progress,
    update_marker_path, Dispatcher, EventFrame, IpcClient, RpcError,
};
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
                Err(RpcError::new(-32601, format!("method not found: {method}")))
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

#[test]
fn rpc_error_serde_roundtrip_keeps_data() {
    // data 在场:序列化携带、反序列化还原(kind 往返命中)
    let err = RpcError::with_data(1, "远端: NOT_FOUND /x", json!({ "kind": "NOT_FOUND" }));
    let wire = serde_json::to_value(&err).unwrap();
    assert_eq!(wire["code"], 1);
    assert_eq!(wire["data"]["kind"], "NOT_FOUND", "data 必须上线: {wire}");
    let back: RpcError = serde_json::from_value(wire).unwrap();
    assert_eq!(back.kind(), Some("NOT_FOUND"), "data 往返必须还原");

    // data 缺省:序列化省略字段
    let plain = RpcError::new(-32601, "method not found: nope");
    let wire = serde_json::to_value(&plain).unwrap();
    assert!(wire.get("data").is_none(), "无 data 时字段必须省略: {wire}");
    let back: RpcError = serde_json::from_value(wire).unwrap();
    assert!(back.data.is_none());

    // 旧形态报文(无 data 字段)反序列化兼容 → None
    let legacy: RpcError =
        serde_json::from_value(json!({ "code": 3, "message": "设备离线" })).unwrap();
    assert!(legacy.data.is_none(), "旧报文缺 data → None");
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

// ---------- 更新标记助手(Task 12 / D11) ----------

/// 标记助手:路径形态钉死({home}/update-in-progress、{home}/restart-ui);
/// 缺失 → None;写入 → Some(内容原样);读失败(路径为目录)按 None——
/// 「不存在」与「读不了」同口径,守卫面不得因坏标记 panic。
#[test]
fn marker_helpers_roundtrip_and_missing() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(
        update_marker_path(home.path()),
        home.path().join("update-in-progress"),
        "更新标记路径形态钉死"
    );
    assert_eq!(
        restart_ui_marker_path(home.path()),
        home.path().join("restart-ui"),
        "restart-ui 标记路径形态钉死"
    );
    assert_eq!(update_in_progress(home.path()), None, "缺失 → None");

    std::fs::write(update_marker_path(home.path()), "1.2.3").unwrap();
    assert_eq!(
        update_in_progress(home.path()),
        Some("1.2.3".to_string()),
        "写入后读回应为标记内容(目标版本)"
    );

    // 读失败(标记路径是目录)按 None:守卫与开机消费都不得 panic
    let bad = tempfile::tempdir().unwrap();
    std::fs::create_dir(update_marker_path(bad.path())).unwrap();
    assert_eq!(update_in_progress(bad.path()), None, "读失败按 None");
}

/// 连接计数(serve 每连接 ±1):客户端连接期间 ≥1,断开后归零。
/// 计数是进程级全局,与本二进制其他并发测试的短命连接会互相叠加——
/// 整段「归零 → 连接 ≥1 → 断开归零」可重试(其他测试的连接终会退场,
/// 重试必然收敛),单轮内每步有界。
#[tokio::test]
async fn connected_clients_tracks_connection_lifecycle() {
    let home = tempfile::tempdir().unwrap();
    let _tx = spawn_server(home.path());
    for _ in 0..10 {
        if !wait_clients_zero(Duration::from_secs(2)).await {
            continue; // 背景连接未退清:换一轮
        }
        let c = connect_retry(home.path()).await;
        // 一次往返:响应回来即证明服务端已 accept(计数在 accept 后立即 +1)
        c.call("echo", json!({})).await.unwrap();
        assert!(
            connected_clients() >= 1,
            "客户端连接期间计数必须 ≥1,得 {}",
            connected_clients()
        );
        drop(c);
        if wait_clients_zero(Duration::from_secs(2)).await {
            return; // 断开后归零:契约达成
        }
    }
    panic!("connected_clients 未能在重试窗内完成 0 → ≥1 → 0 生命周期");
}

/// 轮询直到连接计数归零(或预算耗尽)。
async fn wait_clients_zero(budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if connected_clients() == 0 {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    connected_clients() == 0
}

// ---------- 版本位一致性(1.0.0 起:发版四处同值,漏一即红) ----------

/// 四处版本位一致性(读文件比较):根 Cargo.toml `[workspace.package]`
/// version、ui/src-tauri/Cargo.toml `[package]` version、
/// ui/src-tauri/tauri.conf.json version、ui/package.json version 必须同值。
/// repo 根由 CARGO_MANIFEST_DIR 上溯两级定位(crates/ipc → repo 根)。
#[test]
fn version_positions_are_consistent() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../");
    let root_cargo =
        std::fs::read_to_string(format!("{root}Cargo.toml")).expect("读根 Cargo.toml");
    let tauri_conf: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{root}ui/src-tauri/tauri.conf.json"))
            .expect("读 tauri.conf.json"),
    )
    .expect("tauri.conf.json 应为合法 JSON");
    let ui_cargo = std::fs::read_to_string(format!("{root}ui/src-tauri/Cargo.toml"))
        .expect("读 ui/src-tauri/Cargo.toml");
    let pkg: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{root}ui/package.json")).expect("读 ui/package.json"),
    )
    .expect("ui/package.json 应为合法 JSON");

    let ws = section_version(&root_cargo, "[workspace.package]")
        .expect("根 Cargo.toml [workspace.package] 缺 version");
    let ui = section_version(&ui_cargo, "[package]")
        .expect("ui/src-tauri/Cargo.toml [package] 缺 version");
    let conf = tauri_conf["version"].as_str().expect("tauri.conf.json 缺 version");
    let npm = pkg["version"].as_str().expect("ui/package.json 缺 version");

    assert_eq!(ws, ui, "ui/src-tauri/Cargo.toml 版本与 workspace 不一致");
    assert_eq!(ws, conf, "tauri.conf.json 版本与 workspace 不一致");
    assert_eq!(ws, npm, "ui/package.json 版本与 workspace 不一致");
}

/// 从 Cargo.toml 文本取指定 section 内的 `version = "…"`(无 TOML 依赖的
/// 最小解析:定位 section 标题行,截到下一个行首 `[`,取 version 行引号内值)。
fn section_version<'a>(text: &'a str, section: &str) -> Option<&'a str> {
    let sec_start = text.find(section)?;
    let rest = &text[sec_start..];
    let end = rest[section.len()..]
        .find("\n[")
        .map_or(rest.len(), |i| section.len() + i);
    for line in rest[..end].lines() {
        let line = line.trim();
        let Some(v) = line.strip_prefix("version") else { continue };
        let Some(v) = v.trim_start().strip_prefix('=') else { continue };
        let v = v.trim().strip_prefix('"')?;
        return v.split('"').next();
    }
    None
}
