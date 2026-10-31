//! ios-bridge 契约测试(v2-P5 T2 简 Step 1)。
//!
//! 面:①起桥(mtdss_ios_start 以 tempdir 为 home)→ 轮询连 UDS 发
//! `hello` 得 ok 形;②catch_unwind 不蔓延(桥内 API,#[cfg(test)] 暴露,
//! 落 src/lib.rs 单测);③`mtdss_ios_ipc_path` 输出与 ipc::socket_path
//! 派生一致;④授权根守卫的桥注入端到端(bridge 起引擎后,IPC send 面
//! 集内放行/集外与 `..` 逃逸拒绝)。
//!
//! UDS → Unix only:Windows 命名管道形态不可移植,本文件整体 `#[cfg(unix)]`
//! ——Windows 宿主用 WSL 跑(与 daemon e2e 同法)。
//!
//! 引擎槽位是进程级单例(桥的运行时形态即单引擎),起引擎的用例共用
//! [`ENGINE_LOCK`] 串行化,规避并发 start 撞 `ERR_START_RUNNING`。

#![cfg(unix)]

use mtdss_ipc::IpcClient;
use serde_json::json;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 起引擎用例的进程级串行化(引擎槽位全局唯一,见模块注释)。
static ENGINE_LOCK: Mutex<()> = Mutex::new(());

/// 单次 RPC 收读上界(call 内部无时限,防悬挂显式收口)。
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// 引擎 UDS 就绪轮询窗(daemon READY_TIMEOUT 同量级再放宽)。
const READY_BUDGET: Duration = Duration::from_secs(15);

fn cstring(p: &Path) -> CString {
    CString::new(p.to_str().expect("home 为 UTF-8")).expect("home 无 NUL")
}

/// 轮询连 UDS 至就绪(daemon 由引擎线程拉起,启动有窗口)。
async fn connect_ready(home: &Path) -> IpcClient {
    let deadline = Instant::now() + READY_BUDGET;
    loop {
        match IpcClient::connect(home).await {
            Ok(c) => return c,
            Err(_) => {
                assert!(Instant::now() < deadline, "引擎 UDS 未在窗口内就绪");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

/// 测试①:起桥(tempdir 为 home)→ 连 UDS → `hello` 应答(ok 形:协议
/// 版本在场 + 全新 home 未登录)→ stop 优雅收尾 → UDS 不再可连。
#[tokio::test]
async fn start_bridge_then_hello_over_uds() {
    let _g = ENGINE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let home_c = cstring(home.path());
    let rc = ios_bridge::mtdss_ios_start(home_c.as_ptr());
    assert_eq!(rc, 0, "start 应报已起线程(0),得 {rc}");

    let client = connect_ready(home.path()).await;
    let hello = tokio::time::timeout(CALL_TIMEOUT, client.call("hello", json!({})))
        .await
        .expect("hello 调用不悬挂")
        .expect("hello 调用成功");
    assert!(
        hello["version"].is_string(),
        "hello 应报协议版本: {hello}"
    );
    assert_eq!(
        hello["logged_in"], json!(false),
        "全新 home 应未登录: {hello}"
    );

    assert_eq!(ios_bridge::mtdss_ios_stop(), 0, "stop 应优雅收尾(0)");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if IpcClient::connect(home.path()).await.is_err() {
            break;
        }
        assert!(Instant::now() < deadline, "stop 后 UDS 应不可连");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 测试③:`mtdss_ios_ipc_path` 与 ipc::socket_path 同源一致——null 缓冲/
/// 小缓冲返回所需长度(含 NUL)不写穿;够长即写出且逐字节同派生;null
/// 入参 -1。
#[test]
fn ipc_path_matches_ipc_derivation() {
    let home = tempfile::tempdir().unwrap();
    let home_c = cstring(home.path());
    let expected = mtdss_ipc::socket_path(home.path());
    let needed = ios_bridge::mtdss_ios_ipc_path(home_c.as_ptr(), std::ptr::null_mut(), 0);
    assert_eq!(
        needed as usize,
        expected.len() + 1,
        "null 缓冲应返回所需长度(含 NUL)"
    );

    let mut buf = vec![0 as c_char; expected.len() + 1];
    let rc = ios_bridge::mtdss_ios_ipc_path(home_c.as_ptr(), buf.as_mut_ptr(), buf.len());
    assert_eq!(rc as usize, expected.len() + 1, "够长应写出并返回所需长度");
    let got = unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_str()
        .expect("写出为 UTF-8");
    assert_eq!(got, expected, "派生须与 ipc::socket_path 逐字节一致");

    let mut small = [0 as c_char; 4];
    let rc2 = ios_bridge::mtdss_ios_ipc_path(home_c.as_ptr(), small.as_mut_ptr(), small.len());
    assert_eq!(rc2 as usize, expected.len() + 1, "小缓冲仍返回所需长度");
    assert!(small.iter().all(|&c| c == 0), "缓冲不足不得写");

    assert_eq!(
        ios_bridge::mtdss_ios_ipc_path(std::ptr::null(), std::ptr::null_mut(), 0),
        -1,
        "null home 入参报 -1"
    );
}

/// 测试④:桥注入的授权根(iOS 形态:收件箱 = 默认收件目录)对 IPC send
/// 面生效——集内源放行(落到未登录业务检查)、集外与 `..` 逃逸拒绝(守卫
/// 先于登录检查)。引擎经桥起(daemon 侧收口面由 daemon 单测钉矩阵,此处
/// 钉「桥注入 → opts → 守卫生效」的接线)。
#[tokio::test]
async fn bridge_injects_authorized_roots_send_face_guard() {
    let _g = ENGINE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let inbox = home.path().join("inbox");
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::write(inbox.join("ok.txt"), b"x").unwrap();
    std::fs::write(home.path().join("evil.txt"), b"x").unwrap();

    let rc = ios_bridge::mtdss_ios_start(cstring(home.path()).as_ptr());
    assert_eq!(rc, 0, "start 应报已起线程(0),得 {rc}");
    let client = connect_ready(home.path()).await;

    let send = |files: Vec<String>| {
        let client = &client;
        async move {
            tokio::time::timeout(
                CALL_TIMEOUT,
                client.call(
                    "transfer.start",
                    json!({
                        "kind": "upload",
                        "files": files,
                        "to": "dev-x",
                        "dir": "/tmp/dest-x",
                    }),
                ),
            )
            .await
            .expect("send 调用不悬挂")
        }
    };

    // 集内:守卫放行 → 落到「未登录」业务错(引擎面后续检查)
    let err = send(vec![inbox.join("ok.txt").to_string_lossy().into_owned()])
        .await
        .unwrap_err();
    assert!(
        err.message.contains("未登录"),
        "集内源应过守卫直达登录检查: {err:?}"
    );

    // 集外:守卫拒绝(先于登录检查)
    let err = send(vec![home.path().join("evil.txt").to_string_lossy().into_owned()])
        .await
        .unwrap_err();
    assert!(err.message.contains("授权"), "集外源应被守卫拒绝: {err:?}");

    // `..` 逃逸:形似集内,规范化后落集外,必拒
    let err = send(vec![format!("{}/../evil.txt", inbox.to_string_lossy())])
        .await
        .unwrap_err();
    assert!(err.message.contains("授权"), "`..` 逃逸应被守卫拒绝: {err:?}");

    assert_eq!(ios_bridge::mtdss_ios_stop(), 0, "收尾 stop 应优雅(0)");
}
