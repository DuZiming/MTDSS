//! IPC 契约 golden 锚(v2-P3 Task 3):可移植 RPC 面 + 事件帧逐字节钉死在
//! `crates/ipc/contract/*.json`——目录即单一真源,Kotlin 客户端(T8)读同
//! 一路径作解析夹具,Rust/Kotlin 双端契约同源锁定。
//!
//! 机制(**regenerate 途径**):
//! - 常规跑:帧与 golden 逐字节比对(帧 = 线上 NDJSON 行,含尾换行);
//!   缺失/漂移即红,报文指引重生成。
//! - `REGEN=1 cargo test -p mtdss-daemon --test contract_golden`:重写全部
//!   golden。幂等——fixture 全确定,REGEN 两次无 git diff;生成后以普通
//!   模式复跑确认绿再提交。
//!
//! fixture(确定性设计;golden 在 Windows / Linux CI 逐字节一致):
//! - **固定身份种子**:`identity.seed` 预置固定 32 字节,device_id 由此确定
//!   (sha256(pubkey) 前 32 hex);登录桩回显同一 device_id,upload 驱动内
//!   「身份与会话一致」的核对因此通过,作业得以进入重试循环(非终态窗口
//!   ≥ 重试预算全程,pause/resume/cancel 的成功面稳定)。
//! - **固定端口登录桩**(axum `/auth/*` 全族固定回包;真信令服务器的验证码
//!   是随机六位数、真上线涉及时序,皆不可逐字节复现):`/auth/request-code`
//!   回固定验证码;`/auth/verify` 按**调用计数**回不同形态(1=旧 ok 形态
//!   〔缺 status 字段——同时钉死旧服务器容错的投影〕,2/3=approval 两会话
//!   号,4=limit,5+=新 ok 形态);`/auth/login-status` 按会话号分派
//!   (sess-a/sess-b → ok 带票,其余 expired);`/auth/approve-code` sess-a
//!   → approved 其余 404;`/auth/evict` 恒 ok 带票;`/auth/qr-begin` 回固定
//!   token;`/auth/qr-status` 按**轮询计数**翻面(首查 pending,此后 ok 带
//!   票);`/auth/approval-respond` 回 `{}`;`/auth/qr-claim` 固定候选面
//!   (Bearer 头在断言;死码 404)+ `/auth/qr-approve` 固定 token 受理其余
//!   409(Ruling R8 手机侧透传);`/data/{key}` 固定键值 + 未命
//!   404;`/devices` 回固定目录样本(platform 字段是透传样本,刻意不取宿主
//!   OS)。桩无 `/ws` → 在线循环恒握手失败 → daemon 停在**已登录且离线**
//!   的稳定态(status.online=false;对端面恒「设备离线」code 3,与
//!   CtrlErr 口径一致)。
//! - **预置 tasks.json** 两条终态记录:failed upload(带 rels,reinitiate
//!   成功路径的原料)+ cancelled download(空 rels,clear 走直接删记录
//!   路径);记录 id 同时播种作业计数器(Jobs::new 取 max job_seq + 1 = 3,
//!   后续 mint 序号全确定:upload "3" → reinitiate "4" → remote_copy
//!   `<本机>:5`)。
//! - **路径样本全为固定字符串**:远端目录用 Unix 形(`remote_absolute_form`
//!   双平台认可);上传源用相对路径(cargo test 的 CWD = 包根,正斜杠双平台
//!   一致)。download/local_copy 的 `dir` 走本机平台判定(`absolute_form`),
//!   成功形态天然平台分歧——本锚以平台稳定的「dir 非法」错误覆盖其参数面
//!   与错误契约,成功形态由 T6 的 `{op}.resp.android.json` 变体补。
//! - 核查过 dispatch 全臂:无任何 op 响应嵌时间戳 → 无需固定时间源。
//!
//! 事件帧:brief 钉死的五个方法各一样例帧。event.login / event.transfer 为
//! **真捕获**(登录翻转、作业建队时 daemon 真广播的线上帧);event.online /
//! event.devices / event.notify 按发射点同形构造(hub.rs / flow.rs 的 json!
//! 形状;其中 event.devices 的目录列表取自真 devices RPC 响应——同一
//! build_snapshot 投影的输出,非凭空样本)。
//!
//!
//! `transfer.start.upload-bridge` 形态(v2-P3 T4 已落地):`source:"bridge"`
//! 携 `items` 元数据;受理面(建卡 job_id)双平台逐字节一致——Windows
//! 无 fdpass 通道,驱动起步即「源不可用」失败,响应面不受影响。
//! v2-P3 T6 落地的变体面:`status.resp.android.json`(recv_dir 投影为
//! 收件箱哨兵,与 api.rs 的平台参数化投影同源)、`set_recv_dir.req/
//! resp.json` + `set_recv_dir.resp.android.json`(安卓端收件箱固定的
//! 业务错误,文案常量与 api.rs android 分支共用同一导出)、
//! `transfer.start.download-inbox`(桌面 host 对收件箱哨兵 dir 的拒绝
//! 形态——防串台守卫;android 端的接受面是 cfg 分支,成功形态需 android
//! host,留待实机)。
//!
//! P4a 评估注记(T4,零 golden 变更):`fs.publish` 事件与 fdpass `0x05
//! publish_result` **不入 golden**——前者是引擎侧协议事件,只在真实传输
//! 网络面收妥 finalize 后发射(本 fixture 停在离线稳定态,场景不可达,
//! 同 v2-P3 `fs.request_dest` 先例);后者是 fdpass 旁路线格式,本非
//! JSON-RPC op 面,字节由 `crates/ipc/tests/fdpass_test.rs`(Rust)与
//! Kotlin FdWire 测试双端钉死。`transfer.list` 的 `publishing` 态同理
//! 不入:fixture 三重不可达(预置记录的非终态加载即翻 interrupted;
//! 离线桩无 live 传输进不了发布窗;Windows host 收件箱轮 cfg(unix) 门,
//! download-inbox golden 即其拒收形态)——且 publishing 行与既有行
//! **同形**(state 字符串字段,无新增字段),渲染契约由 Kotlin JVM
//! (TasksReducer)钉,无需 android 变体。桌面侧任务态本就不含
//! publishing(收件箱轮才进入发布窗),golden 桌面面零漂移。

use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Json;
use interprocess::local_socket::tokio::{RecvHalf, SendHalf, Stream};
use interprocess::local_socket::{traits::tokio::Stream as _, GenericFilePath, ToFsName};
use mtdss_core::identity::DeviceIdentity;
use mtdss_core::transport::intake::INBOX_SENTINEL;
use mtdss_daemon::{Daemon, DaemonOpts, ANDROID_SET_RECV_DIR_ERR};
use mtdss_ipc::socket_path;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// golden 目录(单一真源;T8 Kotlin 测试挂同路径为测试资源)。
const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ipc/contract");
/// 登录桩固定端口:服务器 URL 会进 status.resp 与 login.request_code.req,
/// 端口必须固定才能逐字节稳定(占用冲突时测试显式失败,改常量重生成)。
const STUB_PORT: u16 = 39731;
/// 登录桩地址(= session.server 的固定样本)。
const STUB_SERVER: &str = "http://127.0.0.1:39731";
/// 本机固定身份种子(任意 32 字节;device_id/jwt 样本面由此确定)。
const SELF_SEED: [u8; 32] = [0x11; 32];
/// 对端目录条目的公钥种子(任意有效 Ed25519 键;对端不在场,仅目录面)。
const PEER_SEED: [u8; 32] = [0x22; 32];
/// 对端设备 id(目录固定样本;恒离线 → 对端面恒 code 3)。
const PEER_DEV: &str = "dev-peer-fixed";
/// remote_copy 的目标设备(注册表不查目录,无需在目录样本里)。
const PEER2_DEV: &str = "dev-peer2-fixed";
/// 上传源文件:相对包根的固定路径(正向斜杠双平台一致;4096 字节钉死
/// files_total/bytes_total 的预扫计数)。
const FIXTURE_DIR: &str = "contract-fixture";
const FIXTURE_REL: &str = "contract-fixture/契约样本.bin";
const FIXTURE_SIZE: usize = 4096;
/// 登录样本参数(全固定)。
const LOGIN_EMAIL: &str = "contract@example.com";
const LOGIN_CODE: &str = "654321";
const LOGIN_NAME: &str = "契约本机";
/// 审批会话固定样本(桩 login-status/approve-code 按会话号分派)。
const SESS_A: &str = "sess-contract-a";
const SESS_B: &str = "sess-contract-b";
/// 审批到期时刻固定样本。
const SESS_EXPIRES: i64 = 1_760_000_200;
/// 踢选凭据固定样本(verify#4 limit 形态与 login.evict 收口)。
const ENROLL_TOKEN: &str = "enroll-contract-fixed";
/// 扫码会话固定样本(qr_begin 铸定,qr_status 轮询翻面)。
const QR_TOKEN: &str = "qrt-contract-fixed";
const QR_EXPIRES: i64 = 1_760_000_300;
/// 死码样本(不存在/已过期的 404 面;服务端惰性落库后二者同响应)。
const QR_DEAD: &str = "qrt-contract-dead";
/// 扫码候选面固定样本(claim 确认弹层;platform 刻意不取宿主 OS,与
/// 目录样本同一口径——golden 双平台逐字节一致)。
const QR_CANDIDATE_NAME: &str = "契约新机";
/// 账号固定样本(ok 新形态带 account_id;daemon 不投影,仅形态完整)。
const ACCT_ID: &str = "acct-contract";
/// KV 透传样本(键/值/updated_at 全固定)。
const KV_KEY: &str = "常用目录";
const KV_VALUE: &str = "{\"dirs\":[\"/工作\"]}";
const KV_UPDATED: i64 = 1_760_000_400;
/// status 的 recv_dir 固定样本(字符串原样透传,不校验存在性)。
const RECV_DIR_SAMPLE: &str = "/mtdss/contract-inbox";
/// 单次 RPC 收读上界(防悬挂;所有覆盖 op 均为快操作)。
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// 事件排空的静默判定窗与总上界(建队/翻转帧在触发点即广播,静默即全到)。
const DRAIN_QUIET: Duration = Duration::from_millis(400);
const DRAIN_CAP: Duration = Duration::from_secs(5);
/// 整场景上界(含 daemon 启动就绪等待)。
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(90);

// ---------- 裸 NDJSON 客户端(镜像 crates/ipc/tests/ipc.rs 的裸客户端样板) ----------

/// 与 daemon 的真实 IPC 连接:请求帧写出、应答帧读回、通知帧原样缓冲。
/// golden 即这条连接上的逐字节线上帧——锁的是 Kotlin 客户端将要解析的
/// 真实线格式(含 id 回显与事件帧的 jsonrpc 前缀),不是重装出来的形状。
struct RawClient {
    sh: SendHalf,
    read: BufReader<RecvHalf>,
    /// 读应答途中与排空阶段收下的通知帧原样字节(线上行,含尾换行)。
    events: Vec<Vec<u8>>,
}

impl RawClient {
    /// 连接 home 派生的本地套接字(daemon 已由 Daemon::start 拉起就绪)。
    async fn connect(home: &Path) -> Self {
        let name = socket_path(home);
        let stream = Stream::connect(
            name.as_str()
                .to_fs_name::<GenericFilePath>()
                .expect("套接字名转换"),
        )
        .await
        .expect("连接 daemon IPC");
        let (rh, sh) = stream.split();
        Self {
            sh,
            read: BufReader::new(rh),
            events: Vec::new(),
        }
    }

    /// 发一帧请求(id 恒 1)→ 读回同 id 应答;途中的通知帧(无 id)入缓冲。
    /// 返回 (请求行字节, 应答行字节),含尾换行,即两个 golden 文件的内容。
    async fn rpc(&mut self, method: &str, params: Value) -> (Vec<u8>, Vec<u8>) {
        // 与 IpcClient::call 同款帧形状(jsonrpc/id/method/params)
        let req = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let mut line = serde_json::to_string(&req)
            .expect("请求序列化")
            .into_bytes();
        line.push(b'\n');
        self.sh.write_all(&line).await.expect("写请求帧");
        self.sh.flush().await.expect("flush");
        let resp = tokio::time::timeout(CALL_TIMEOUT, async {
            loop {
                let mut buf = String::new();
                let n = self.read.read_line(&mut buf).await.expect("读应答行");
                assert!(n > 0, "IPC 连接在读应答时关闭({method})");
                let v: Value = serde_json::from_str(buf.trim_end())
                    .unwrap_or_else(|e| panic!("应答行非合法 JSON({e}): {buf}"));
                if v.get("id").and_then(Value::as_i64) == Some(1) {
                    let mut bytes = buf.into_bytes();
                    if bytes.last() != Some(&b'\n') {
                        bytes.push(b'\n');
                    }
                    return bytes;
                }
                self.events.push(buf.into_bytes());
            }
        })
        .await
        .unwrap_or_else(|_| panic!("等待 {method} 应答超时({CALL_TIMEOUT:?})"));
        (line, resp)
    }

    /// 排空事件流直至静默:此前已广播的帧全部落进缓冲(捕获按谓词过滤,
    /// 迟到的无关帧不构成干扰)。
    async fn drain(&mut self) {
        let deadline = tokio::time::Instant::now() + DRAIN_CAP;
        loop {
            if tokio::time::Instant::now() >= deadline {
                return;
            }
            let mut buf = String::new();
            match tokio::time::timeout(DRAIN_QUIET, self.read.read_line(&mut buf)).await {
                Err(_) => return, // 静默窗耗尽:该到的帧已到
                Ok(Ok(0)) => return,
                Ok(Ok(_)) => self.events.push(buf.into_bytes()),
                Ok(Err(e)) => panic!("排空事件流失败: {e}"),
            }
        }
    }
}

// ---------- 登录桩(固定端口,/auth/* 全族固定回包) ----------

/// 起登录桩:`/auth/*` 各路由固定回包 + `/devices` 固定目录;其余路径(含
/// 在线循环探测的 `/ws`)一律 404——握手失败即离线稳定态。形态分派见模块
/// 注释的登录桩节(verify 按调用计数、login-status/ approve-code 按会话
/// 号、qr-status 按轮询计数翻面;全部全确定,REGEN 幂等)。
async fn spawn_login_stub(self_id: String, devices: Value) -> tokio::task::JoinHandle<()> {
    let verify_calls = Arc::new(AtomicUsize::new(0));
    let qr_polls = Arc::new(AtomicBool::new(false));
    // ok 带票形态的固定令牌/身份(verify#5+ / login-status / evict / qr ok
    // 同一票面:续接收口一律落到同一会话,与 verify#1 的 device_id 一致)
    let ok_form = |self_id: String| {
        json!({
            "status": "ok",
            "jwt": "contract-access-token",
            "refresh": "contract-refresh-token",
            "device_id": self_id,
            "account_id": ACCT_ID,
        })
    };
    let self_legacy = self_id.clone(); // verify#1 旧形态(缺 status)
    let self_new = self_id.clone(); // verify#4 limit 清单/verify#5+ ok
    let self_status_ls = self_id.clone(); // login-status ok 带票
    let self_status_evict = self_id.clone(); // evict ok 带票
    let self_status_qr = self_id; // qr-status ok 带票
    let verify_stub = verify_calls.clone();
    let qr_stub = qr_polls.clone();
    let app = axum::Router::new()
        .route(
            "/auth/request-code",
            post(|| async { Json(json!({ "code": LOGIN_CODE })) }),
        )
        .route(
            "/auth/verify",
            post(move |Json(_): Json<Value>| async move {
                let n = verify_stub.fetch_add(1, Ordering::SeqCst) + 1;
                match n {
                    // 旧服务器形态(缺 status 字段):钉死 daemon 容错投影
                    1 => Json(json!({
                        "jwt": "contract-access-token",
                        "refresh": "contract-refresh-token",
                        "device_id": self_legacy,
                    })),
                    2 => Json(json!({
                        "status": "approval", "session_id": SESS_A, "expires_at": SESS_EXPIRES,
                    })),
                    3 => Json(json!({
                        "status": "approval", "session_id": SESS_B, "expires_at": SESS_EXPIRES,
                    })),
                    4 => Json(json!({
                        "status": "limit", "enroll_token": ENROLL_TOKEN,
                        "devices": [
                            {"device_id": self_new.clone(), "name": LOGIN_NAME, "platform": "windows", "online": false},
                            {"device_id": PEER_DEV, "name": "契约对端", "platform": "linux", "online": false},
                        ],
                    })),
                    _ => Json(ok_form(self_new.clone())),
                }
            }),
        )
        .route(
            "/auth/approve-code",
            post(|Json(v): Json<Value>| async move {
                if v["session_id"].as_str() == Some(SESS_A) {
                    Json(json!({ "status": "approved" })).into_response()
                } else {
                    (axum::http::StatusCode::NOT_FOUND, "unknown session".to_string())
                        .into_response()
                }
            }),
        )
        .route(
            "/auth/login-status",
            post(move |Json(v): Json<Value>| {
                let self_id = self_status_ls.clone();
                async move {
                    match v["session_id"].as_str() {
                        s if s == Some(SESS_A) || s == Some(SESS_B) => Json(ok_form(self_id)),
                        _ => Json(json!({ "status": "expired" })),
                    }
                }
            }),
        )
        .route(
            "/auth/evict",
            post(move |Json(_): Json<Value>| {
                let self_id = self_status_evict.clone();
                async move { Json(ok_form(self_id)) }
            }),
        )
        .route(
            "/auth/qr-begin",
            post(|| async { Json(json!({ "qr_token": QR_TOKEN, "expires_at": QR_EXPIRES })) }),
        )
        .route(
            "/auth/qr-status",
            get(move |axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>| {
                let qr_stub = qr_stub.clone();
                let self_id = self_status_qr.clone();
                async move {
                    assert_eq!(
                        q.get("t").map(String::as_str),
                        Some(QR_TOKEN),
                        "qr-status 桩只服务固定 token"
                    );
                    let first = !qr_stub.swap(true, Ordering::SeqCst);
                    if first {
                        Json(json!({ "status": "pending", "expires_at": QR_EXPIRES }))
                    } else {
                        Json(ok_form(self_id))
                    }
                }
            }),
        )
        .route(
            "/auth/approval-respond",
            post(|| async { Json(json!({})) }),
        )
        .route(
            "/auth/qr-claim",
            post(
                |headers: axum::http::HeaderMap, Json(v): Json<Value>| async move {
                    assert!(
                        headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|h| h.to_str().ok())
                            .map(|h| h.starts_with("Bearer ") && h.len() > "Bearer ".len())
                            .unwrap_or(false),
                        "qr-claim 须携 Bearer(daemon 会话 JWT 代打)"
                    );
                    match v["qr_token"].as_str() {
                        Some(t) if t == QR_TOKEN => Json(json!({
                            "candidate": {"name": QR_CANDIDATE_NAME, "platform": "windows"},
                            "expires_at": QR_EXPIRES,
                        }))
                        .into_response(),
                        _ => (
                            axum::http::StatusCode::NOT_FOUND,
                            "unknown qr token".to_string(),
                        )
                            .into_response(),
                    }
                },
            ),
        )
        .route(
            "/auth/qr-approve",
            post(|Json(v): Json<Value>| async move {
                match v["qr_token"].as_str() {
                    // 200 = 受理(裁决方向由 allow 落库,响应体恒空对象)
                    Some(t) if t == QR_TOKEN => Json(json!({})).into_response(),
                    // 已 settle/已过期(单次裁决重放与死码同 409 面)
                    _ => (
                        axum::http::StatusCode::CONFLICT,
                        "already settled or expired".to_string(),
                    )
                        .into_response(),
                }
            }),
        )
        .route(
            "/data/{key}",
            get(|axum::extract::Path(key): axum::extract::Path<String>| async move {
                if key == KV_KEY {
                    Json(json!({ "value": KV_VALUE, "updated_at": KV_UPDATED })).into_response()
                } else {
                    (axum::http::StatusCode::NOT_FOUND, "no such key".to_string()).into_response()
                }
            })
            .put(|axum::extract::Path(key): axum::extract::Path<String>| async move {
                assert_eq!(key, KV_KEY, "kv 桩只服务固定键");
                Json(json!({ "updated_at": KV_UPDATED }))
            }),
        )
        .route("/devices", get(move || async move { Json(devices) }));
    let listener = reuse_listener(STUB_PORT).expect("绑定登录桩固定端口(被占则换常量)");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("登录桩 serve 不应退出");
    })
}

/// SO_REUSEADDR 监听器(沿 online.rs 同款:重复跑测试绕过 TIME_WAIT 残留,
/// Windows 占端口语义)。
fn reuse_listener(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let sock = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    sock.set_reuse_address(true)?;
    sock.set_nonblocking(true)?;
    sock.bind(&addr.into())?;
    sock.listen(1024)?;
    tokio::net::TcpListener::from_std(sock.into())
}

// ---------- fixture 预置 ----------

/// fixture 源文件:包根下 `contract-fixture/契约样本.bin`(4096 字节);
/// Drop 清理,panic 也不残留工作树。相对路径入 req golden(cargo test 的
/// CWD = 包根,进程内 daemon 同 CWD 可解析)。
struct FixtureDir(PathBuf);

impl FixtureDir {
    fn create() -> Self {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_DIR);
        std::fs::create_dir_all(&dir).expect("建 fixture 目录");
        std::fs::write(dir.join("契约样本.bin"), vec![0u8; FIXTURE_SIZE])
            .expect("写 fixture 源文件");
        Self(dir)
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// home 预置:固定身份种子 + 两条终态任务记录(见模块注释的 fixture 节)。
fn preset_home(home: &Path, self_id: &str) {
    DeviceIdentity::from_seed(&SELF_SEED)
        .save_seed(&home.join("identity.seed"))
        .expect("写固定身份种子");
    // 记录字段面与 TaskStore::load 的解析一一对应(serde default 容器);
    // 时间戳为固定样本(不进任何 RPC 投影,仅供记录自洽)。
    let recs = json!([
        {
            "id": "1", "kind": "upload", "name": "契约样本.bin",
            "from_dev": self_id, "to_dev": PEER_DEV,
            "src_paths": [FIXTURE_REL], "target_dir": "/golden/peer-inbox",
            "policy_skip": [], "policy_overwrite": [], "cut": false,
            "rels": ["契约样本.bin"],
            "files_done": 0, "files_total": 1, "bytes_done": 0, "bytes_total": FIXTURE_SIZE as u64,
            "state": "failed",
            "error": "发送失败(共尝试 6 轮,最后错误: 信令未在线,无法发起发送)",
            "created_ms": 1760000000000u64, "finished_ms": 1760000031000u64,
        },
        {
            "id": "2", "kind": "download", "name": "契约 文档",
            "from_dev": PEER_DEV, "to_dev": self_id,
            "src_paths": ["/远端/契约 文档.pdf"], "target_dir": "/golden/inbox",
            "policy_skip": [], "policy_overwrite": [], "cut": false,
            "rels": [],
            "files_done": 0, "files_total": 0, "bytes_done": 0, "bytes_total": 0,
            "state": "cancelled", "error": null,
            "created_ms": 1760000100000u64, "finished_ms": 1760000120000u64,
        },
    ]);
    std::fs::write(
        home.join("tasks.json"),
        serde_json::to_string_pretty(&recs).expect("任务记录序列化"),
    )
    .expect("写预置 tasks.json");
}

// ---------- 应答解析/断言助手 ----------

/// 帧字节去掉尾换行。
fn line_body(b: &[u8]) -> &[u8] {
    b.strip_suffix(b"\n").unwrap_or(b)
}

/// 应答行的 result(错误应答即 panic,带帧全文——fixture 语义漂移在断言层
/// 先于 golden 比对显形,REGEN 不会静默烤进坏形状)。
fn parse_result(resp: &[u8]) -> Value {
    let v: Value = serde_json::from_slice(line_body(resp)).expect("应答为合法 JSON");
    v.get("result")
        .cloned()
        .unwrap_or_else(|| panic!("预期成功应答: {}", String::from_utf8_lossy(line_body(resp))))
}

/// 应答行的 error → (code, message)。
fn parse_error(resp: &[u8]) -> (i64, String) {
    let v: Value = serde_json::from_slice(line_body(resp)).expect("应答为合法 JSON");
    let e = v
        .get("error")
        .cloned()
        .unwrap_or_else(|| panic!("预期错误应答: {}", String::from_utf8_lossy(line_body(resp))));
    (
        e["code"]
            .as_i64()
            .unwrap_or_else(|| panic!("error.code 缺失: {e}")),
        e["message"].as_str().unwrap_or_default().to_string(),
    )
}

/// 缓冲事件里找首个满足谓词的帧(原样字节)。
fn take_event(events: &[Vec<u8>], what: &str, pred: impl Fn(&Value) -> bool) -> Vec<u8> {
    events
        .iter()
        .find(|b| {
            let v: Value = serde_json::from_slice(line_body(b)).expect("事件帧为合法 JSON");
            pred(&v)
        })
        .cloned()
        .unwrap_or_else(|| panic!("未捕获到 {what}:已缓冲 {} 帧", events.len()))
}

/// 通知帧构造:与 ipc::write_loop 的 json! 形状逐字相同(线上会补
/// "jsonrpc":"2.0" 前缀、无 id)。
fn event_frame(method: &str, params: Value) -> Vec<u8> {
    let mut b = serde_json::to_string(&json!({
        "jsonrpc": "2.0", "method": method, "params": params,
    }))
    .expect("事件帧序列化")
    .into_bytes();
    b.push(b'\n');
    b
}

/// 应答帧构造(id 恒 1,与 RawClient::rpc 的请求帧配对):成功形态。
/// android 变体 golden 的桌面生成面(v2-P3 T6)——序列化走 serde_json
/// Value(键序与 daemon 侧 ipc::handle_frame 同一实现,逐字节一致)。
fn result_resp_frame(result: Value) -> Vec<u8> {
    let mut b = serde_json::to_string(&json!({ "id": 1, "result": result }))
        .expect("应答帧序列化")
        .into_bytes();
    b.push(b'\n');
    b
}

/// 应答帧构造:错误形态(`{"error":{code,message}}`,data 缺省省略——
/// 与 RpcError 的 serde 形状一致)。
fn error_resp_frame(code: i64, message: &str) -> Vec<u8> {
    let mut b = serde_json::to_string(&json!({
        "id": 1,
        "error": { "code": code, "message": message },
    }))
    .expect("应答帧序列化")
    .into_bytes();
    b.push(b'\n');
    b
}

/// 一个 op 的 req/resp 帧入 golden 集。
fn put(g: &mut BTreeMap<String, Vec<u8>>, op: &str, req: Vec<u8>, resp: Vec<u8>) {
    g.insert(format!("{op}.req.json"), req);
    g.insert(format!("{op}.resp.json"), resp);
}

// ---------- 场景:两态驱完全部覆盖 op,收集线上帧 ----------

/// 跑完整 fixture 场景,返回 golden 名 → 帧字节(含尾换行)。
async fn scenario() -> BTreeMap<String, Vec<u8>> {
    // ---- 固定身份与目录样本(全部由固定种子推导,跨机跨平台一致) ----
    let self_id = DeviceIdentity::from_seed(&SELF_SEED);
    let peer_id = DeviceIdentity::from_seed(&PEER_SEED);
    let devices = json!({
        "devices": [
            {
                "device_id": self_id.device_id,
                "pubkey": hex::encode(self_id.public_key().as_bytes()),
                "name": LOGIN_NAME, "platform": "windows", "online": true,
            },
            {
                "device_id": PEER_DEV,
                "pubkey": hex::encode(peer_id.public_key().as_bytes()),
                "name": "契约对端", "platform": "linux", "online": false,
            },
        ]
    });

    // ---- fixture 源文件 + home 预置 + 桩 + daemon ----
    let _fixture = FixtureDir::create();
    assert!(
        std::fs::metadata(FIXTURE_REL).is_ok(),
        "相对 fixture 路径 {FIXTURE_REL} 须可解析(本测试经 cargo test 运行,CWD = 包根)"
    );
    let home = tempfile::tempdir().unwrap();
    preset_home(home.path(), &self_id.device_id);
    let stub = spawn_login_stub(self_id.device_id.clone(), devices).await;
    let daemon = Daemon::start(
        home.path(),
        DaemonOpts {
            recv_dir: PathBuf::from(RECV_DIR_SAMPLE),
            tray: false,
            authorized_roots: None,
            inbox_staging: None,
            // 桌面形态:无收件箱发布面(v2-P5 T5b,哨兵回退默认收件目录)
            inbox_publish: false,
            photo_serve: false,
        },
    )
    .await
    .expect("daemon 启动");
    let mut c = RawClient::connect(home.path()).await;
    let mut g: BTreeMap<String, Vec<u8>> = BTreeMap::new();

    // ---- 登录三联(未登录 → 已登录;桩上逐字节可复现) ----
    let (req, resp) = c
        .rpc(
            "login.request_code",
            json!({ "server": STUB_SERVER, "email": LOGIN_EMAIL }),
        )
        .await;
    assert_eq!(
        parse_result(&resp)["code"].as_str(),
        Some(LOGIN_CODE),
        "桩回显固定验证码"
    );
    put(&mut g, "login.request_code", req, resp);

    let (req, resp) = c
        .rpc(
            "login.verify",
            json!({ "email": LOGIN_EMAIL, "code": LOGIN_CODE, "name": LOGIN_NAME }),
        )
        .await;
    let r = parse_result(&resp);
    assert_eq!(r["device_id"].as_str(), Some(self_id.device_id.as_str()));
    assert_eq!(r["name"].as_str(), Some(LOGIN_NAME));
    put(&mut g, "login.verify", req, resp);

    // ---- 已登录(离线)态的查询面 ----
    let (req, resp) = c.rpc("hello", json!({})).await;
    let r = parse_result(&resp);
    assert_eq!(r["logged_in"], true);
    assert_eq!(r["device"]["device_id"], self_id.device_id.clone());
    assert!(r["version"].as_str().is_some(), "version 恒有值: {r}");
    put(&mut g, "hello", req, resp);

    let (req, resp) = c.rpc("status", json!({})).await;
    let r = parse_result(&resp);
    assert_eq!(r["logged_in"], true);
    assert_eq!(r["online"], false, "桩无 WS,恒离线稳定态");
    assert_eq!(r["server"], STUB_SERVER);
    assert_eq!(r["recv_dir"], RECV_DIR_SAMPLE);
    put(&mut g, "status", req, resp.clone());
    // android 变体(v2-P3 T6):同一 status 的 recv_dir 投影为收件箱哨兵
    // ——真捕获的桌面帧 + 哨兵值(与 api.rs 平台参数化投影同源:android
    // 端其余字段同形,唯一分歧即 recv_dir)。
    let mut android_status = parse_result(&resp);
    android_status["recv_dir"] = json!(INBOX_SENTINEL);
    g.insert(
        "status.resp.android.json".into(),
        result_resp_frame(android_status),
    );

    let (req, resp) = c.rpc("devices", json!({})).await;
    let r = parse_result(&resp);
    assert_eq!(r["stale"], false, "桩现拉命中,stale=false");
    assert_eq!(r["devices"].as_array().map(Vec::len), Some(2));
    let devices_list = r["devices"].clone();
    put(&mut g, "devices", req, resp);

    // ---- 远端浏览面:对端恒离线 → code 3(平台稳定的错误契约) ----
    let (req, resp) = c
        .rpc("list", json!({ "device": PEER_DEV, "path": "/远端/文档" }))
        .await;
    assert_eq!(parse_error(&resp), (3, format!("设备离线: {PEER_DEV}")));
    put(&mut g, "list", req, resp);

    // list resp 条目形态样本(v2-P5 T5b children 增量的桌面生成面,同
    // status.resp.android 的构造帧手法——成功形态需对端在线,fixture 不可
    // 达,但 DirEntry 的线上 JSON 形状可在此逐字节钉死):目录项携
    // `children`(直接子项数,read_dir 全量计数含 dotfiles);文件项无该
    // 键(proto3 optional 缺省 = 整键缺省,与 api.rs dir_entry_json 同一
    // 实现序列化)。Kotlin/Swift 契约测试吃同一文件,三端同源锁定。
    g.insert(
        "list.resp.entries.json".into(),
        result_resp_frame(json!({
            "entries": [
                {
                    "name": "文档 目录", "path": "/远端/文档 目录",
                    "size": 0, "mtime_ms": 1760000000000i64, "is_dir": true,
                    "children": 2,
                },
                {
                    "name": "契约 文档.pdf", "path": "/远端/契约 文档.pdf",
                    "size": 4096, "mtime_ms": 1760000100000i64, "is_dir": false,
                },
            ],
            "truncated": false,
        })),
    );

    let (req, resp) = c
        .rpc(
            "stat",
            json!({ "device": PEER_DEV, "path": "/远端/契约 文档.pdf" }),
        )
        .await;
    assert_eq!(parse_error(&resp), (3, format!("设备离线: {PEER_DEV}")));
    put(&mut g, "stat", req, resp);

    // delete(v2-P5 T5b ③ 入锚):同 list/stat 的对端离线错误契约形态——
    // 参数面(device/paths/force)由 req 帧逐字节钉死;成功形态需对端在线,
    // 与远端浏览面同理留待实机。客户端绑定:Swift 表单化(字节对齐),
    // Kotlin 声明不移植(安卓无远端删除面)。
    let (req, resp) = c
        .rpc(
            "delete",
            json!({
                "device": PEER_DEV,
                "paths": ["/远端/契约 文档.pdf"],
                "force": false,
            }),
        )
        .await;
    assert_eq!(parse_error(&resp), (3, format!("设备离线: {PEER_DEV}")));
    put(&mut g, "delete", req, resp);

    // ---- 传输作业面 ----
    // upload(路径形态):驱动在重试循环里(信令离线 → Network 可重试,
    // 重试预算 5×退避 ≈ 31s),非终态窗口远大于后续 pause/resume/cancel 的
    // 到达时机——三联与 wait 的成功面因此确定。
    let (req, resp) = c
        .rpc(
            "transfer.start",
            json!({
                "kind": "upload", "files": [FIXTURE_REL], "to": PEER_DEV,
                "dir": "/golden/peer-inbox", "skip": [], "overwrite": [], "cut": false,
            }),
        )
        .await;
    let r = parse_result(&resp);
    assert_eq!(r["job_id"].as_str(), Some("3"), "计数器自预置记录播种: {r}");
    put(&mut g, "transfer.start.upload", req, resp);

    let (req, resp) = c.rpc("transfer.pause", json!({ "id": "3" })).await;
    assert_eq!(parse_result(&resp), json!({}), "暂停成功面");
    put(&mut g, "transfer.pause", req, resp);

    let (req, resp) = c.rpc("transfer.resume", json!({ "id": "3" })).await;
    assert_eq!(parse_result(&resp), json!({}), "恢复成功面");
    put(&mut g, "transfer.resume", req, resp);

    let (req, resp) = c.rpc("transfer.cancel", json!({ "id": "3" })).await;
    assert_eq!(parse_result(&resp), json!({}), "取消成功面");
    put(&mut g, "transfer.cancel", req, resp);

    let (req, resp) = c.rpc("transfer.wait", json!({ "id": "3" })).await;
    let r = parse_result(&resp);
    assert_eq!(r["state"], "cancelled", "取消后 wait 即返终态: {r}");
    assert_eq!(r["files_total"], 1, "预扫单文件");
    assert_eq!(r["bytes_total"], FIXTURE_SIZE as u64);
    assert_eq!(r["files_done"], 0, "离线态无进度");
    put(&mut g, "transfer.wait", req, resp);

    // transfer.list:持久层打底(预置记录 1/2 的 task_rec_json 投影)+
    // live 覆盖(作业 3 的 job_json 投影)——一张 golden 锁两种条目形状。
    let (req, resp) = c.rpc("transfer.list", json!({})).await;
    let r = parse_result(&resp);
    let ids: Vec<&str> = r["jobs"]
        .as_array()
        .expect("jobs 数组")
        .iter()
        .map(|j| j["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["1", "2", "3"], "持久 1/2 + live 3,按序号升序");
    assert_eq!(r["jobs"][0]["state"], "failed");
    assert_eq!(r["jobs"][1]["state"], "cancelled");
    assert_eq!(r["jobs"][2]["state"], "cancelled");
    assert_eq!(r["jobs"][2]["speed_bps"], 0, "无进度采样,速度恒 0");
    put(&mut g, "transfer.list", req, resp);

    // reinitiate:预置 failed 记录翻新重发(新作业 "4" 异步驱动,响应面
    // 即成功空对象);clear:预置 cancelled download(空 rels)直接删记录。
    let (req, resp) = c.rpc("transfer.reinitiate", json!({ "id": "1" })).await;
    assert_eq!(parse_result(&resp), json!({}), "重发成功面");
    put(&mut g, "transfer.reinitiate", req, resp);

    let (req, resp) = c.rpc("transfer.clear", json!({ "id": "2" })).await;
    assert_eq!(parse_result(&resp), json!({}), "清除成功面");
    put(&mut g, "transfer.clear", req, resp);

    // download / local_copy:dir 属本机平台判定面(absolute_form),成功形
    // 态平台分歧(见模块注释)——以平台稳定的「dir 非法」错误锁参数面与
    // 错误契约;成功形态由 T6 android 变体补。相对 dir 双平台同判非法。
    let (req, resp) = c
        .rpc(
            "transfer.start",
            json!({
                "kind": "download", "device": PEER_DEV,
                "paths": ["/远端/契约 文档.pdf"], "dir": "golden/inbox",
                "skip": [], "overwrite": [], "cut": false,
            }),
        )
        .await;
    let (code, msg) = parse_error(&resp);
    assert_eq!(code, 1);
    assert!(
        msg.starts_with("dir 非法"),
        "平台稳定的 dir 校验错误: {msg}"
    );
    put(&mut g, "transfer.start.download", req, resp);

    // download dir="inbox"(v2-P3 T6 收件箱哨兵):桌面 host(非 android)
    // 不接受——「dir 非法」既有文案即防串台守卫(android 端的接受面是
    // cfg 分支,不在桌面场景;成功形态需 android host,留待实机)。
    let (req, resp) = c
        .rpc(
            "transfer.start",
            json!({
                "kind": "download", "device": PEER_DEV,
                "paths": ["/远端/契约 文档.pdf"], "dir": "inbox",
                "skip": [], "overwrite": [], "cut": false,
            }),
        )
        .await;
    let (code, msg) = parse_error(&resp);
    assert_eq!(code, 1);
    assert!(
        msg.starts_with("dir 非法"),
        "桌面不接受收件箱哨兵 dir: {msg}"
    );
    put(&mut g, "transfer.start.download-inbox", req, resp);

    let (req, resp) = c
        .rpc(
            "transfer.start",
            json!({
                "kind": "local_copy", "paths": [FIXTURE_REL], "dir": "golden/inbox",
                "cut": false,
            }),
        )
        .await;
    let (code, msg) = parse_error(&resp);
    assert_eq!(code, 1);
    assert!(
        msg.starts_with("dir 非法"),
        "平台稳定的 dir 校验错误: {msg}"
    );
    put(&mut g, "transfer.start.local_copy", req, resp);

    // remote_copy:dir 属对端(平台无关判)→ 成功面可稳定锁定;id 带本机
    // 命名空间(`<本机>:5`)。驱动异步失败(orchestrate 撞对端离线),不
    // 进任何已捕获帧。
    let (req, resp) = c
        .rpc(
            "transfer.start",
            json!({
                "kind": "remote_copy", "from": PEER_DEV, "paths": ["/远端/直传 目录"],
                "to": PEER2_DEV, "dir": "/golden/peer2-inbox",
                "skip": [], "overwrite": [], "cut": false,
            }),
        )
        .await;
    let r = parse_result(&resp);
    assert_eq!(
        r["job_id"].as_str(),
        Some(format!("{}:5", self_id.device_id).as_str()),
        "remote_copy id 带本机命名空间: {r}"
    );
    put(&mut g, "transfer.start.remote_copy", req, resp);

    // upload(bridge 形态,v2-P3 T4):`source:"bridge"` + items 元数据
    // (懒 fd——驱动起步才逐文件 fs.request_source;本场景无 App 应答,
    // 驱动异步失败〔Windows 立即/Linux 10s 超时〕,不进任何已捕获帧;
    // 放在 transfer.list 之后,list golden 不纳入本作业)。计数器续接
    // remote_copy 的 5 → "6"(确定)。
    let (req, resp) = c
        .rpc(
            "transfer.start",
            json!({
                "kind": "upload", "source": "bridge",
                "items": [
                    {
                        "ref": "bridge-sample-ref",
                        "rel": "契约桥接样本.bin",
                        "size": 4096, "mtime": 1760000000000i64,
                    }
                ],
                "to": PEER_DEV, "dir": "/golden/peer-inbox",
                "skip": [], "overwrite": [], "cut": false,
            }),
        )
        .await;
    let r = parse_result(&resp);
    assert_eq!(
        r["job_id"].as_str(),
        Some("6"),
        "计数器续接 remote_copy 的 5: {r}"
    );
    put(&mut g, "transfer.start.upload-bridge", req, resp);

    // set_recv_dir(桌面形态,v2-P3 T6 入锚):固定样本目录,成功空对象。
    // 置于场景尾部(此后仅登出)——state.recv_dir 的变更不回渗此前帧。
    let (req, resp) = c
        .rpc("set_recv_dir", json!({ "dir": "/mtdss/contract-inbox-2" }))
        .await;
    assert_eq!(parse_result(&resp), json!({}), "设置成功面");
    put(&mut g, "set_recv_dir", req, resp);
    // android 变体:同一请求在安卓端回业务错误「收件箱固定」——构造帧
    // 与 api.rs android 分支共用同一文案常量(lib.rs doc(hidden) 导出),
    // 桌面 host 无 android daemon 也能逐字节钉死该形态。
    g.insert(
        "set_recv_dir.resp.android.json".into(),
        error_resp_frame(1, ANDROID_SET_RECV_DIR_ERR),
    );

    // ---- V3-1 登录状态机面(审批/踢选/扫码/KV;桩分派全确定) ----
    // 场景按桩 verify 计数推进:此时已用 #1(登录三联的旧形态直入)。
    // request_code 只为重置待登录槽(响应与已采集的 login.request_code 同
    // 字节,不再重复采集);verify#2/#3/#4 分别回 approval(sess-a)/
    // approval(sess-b)/limit。
    macro_rules! rearm {
        ($c:expr) => {
            let _ = $c
                .rpc(
                    "login.request_code",
                    json!({ "server": STUB_SERVER, "email": LOGIN_EMAIL }),
                )
                .await;
        };
    }

    // verify 审批变体(桩 #2 → sess-a;槽记会话号与候选名)
    rearm!(c);
    let (_req, resp) = c
        .rpc(
            "login.verify",
            json!({ "email": LOGIN_EMAIL, "code": LOGIN_CODE, "name": LOGIN_NAME }),
        )
        .await;
    let r = parse_result(&resp);
    assert_eq!(r["status"], "approval", "桩 #2 必须回审批面: {r}");
    assert_eq!(r["session_id"], SESS_A);
    assert_eq!(r["expires_at"], SESS_EXPIRES);
    g.insert("login.verify.resp.approval.json".into(), resp);

    // 输码收口:approve-code(sess-a)→ approved → 即时轮询领票(ok)→
    // 会话翻转;req 面钉 {session_id, code} 双字段
    let (req, resp) = c
        .rpc(
            "login.submit_code",
            json!({ "session_id": SESS_A, "code": "123456" }),
        )
        .await;
    let r = parse_result(&resp);
    assert_eq!(r["status"], "ok", "输码收口必须领票登录: {r}");
    assert_eq!(r["device_id"].as_str(), Some(self_id.device_id.as_str()));
    assert_eq!(r["name"], LOGIN_NAME);
    put(&mut g, "login.submit_code", req, resp);

    // 轮询收口(桩 #3 → sess-b):approval 变体已钉,直接领票
    rearm!(c);
    let _ = c
        .rpc(
            "login.verify",
            json!({ "email": LOGIN_EMAIL, "code": LOGIN_CODE, "name": LOGIN_NAME }),
        )
        .await;
    let (req, resp) = c.rpc("login.status", json!({ "session_id": SESS_B })).await;
    let r = parse_result(&resp);
    assert_eq!(r["status"], "ok", "轮询收口必须领票登录: {r}");
    put(&mut g, "login.status", req, resp);

    // verify limit 变体(桩 #4):踢选凭据 + 现有清单(本机/对端各一台)
    rearm!(c);
    let (_req, resp) = c
        .rpc(
            "login.verify",
            json!({ "email": LOGIN_EMAIL, "code": LOGIN_CODE, "name": LOGIN_NAME }),
        )
        .await;
    let r = parse_result(&resp);
    assert_eq!(r["status"], "limit", "桩 #4 必须回 limit: {r}");
    assert_eq!(r["enroll_token"], ENROLL_TOKEN);
    assert_eq!(r["devices"].as_array().map(Vec::len), Some(2));
    assert_eq!(r["devices"][0]["device_id"].as_str(), Some(self_id.device_id.as_str()));
    assert_eq!(r["devices"][1]["device_id"], PEER_DEV);
    g.insert("login.verify.resp.limit.json".into(), resp);

    // 踢选收口:evict(ok 带票)→ 会话翻转;ok 投影 = verify ok 形态
    let (req, resp) = c
        .rpc(
            "login.evict",
            json!({ "enroll_token": ENROLL_TOKEN, "device_id": PEER_DEV }),
        )
        .await;
    let r = parse_result(&resp);
    assert_eq!(r["status"], "ok", "踢选收口必须领票登录: {r}");
    put(&mut g, "login.evict", req, resp);

    // 取消:作废本地续接槽(无服务端取消端点,TTL 收口),幂等空对象
    let (req, resp) = c.rpc("login.cancel", json!({})).await;
    assert_eq!(parse_result(&resp), json!({}));
    put(&mut g, "login.cancel", req, resp);

    // 扫码:begin → daemon 拼二维码 URL(固定前缀 + token);status 首查
    // pending,再查 ok 收口(桩按轮询计数翻面)
    let (req, resp) = c
        .rpc("login.qr_begin", json!({ "server": STUB_SERVER, "name": LOGIN_NAME }))
        .await;
    let r = parse_result(&resp);
    assert_eq!(
        r["qr_url"].as_str(),
        Some(format!("https://www.mtdss.top/qr?t={QR_TOKEN}").as_str()),
        "二维码内容由 daemon 拼: {r}"
    );
    assert_eq!(r["token"], QR_TOKEN);
    assert_eq!(r["expires_at"], QR_EXPIRES);
    put(&mut g, "login.qr_begin", req, resp);

    let (_req, resp) = c.rpc("login.qr_status", json!({ "token": QR_TOKEN })).await;
    let r = parse_result(&resp);
    assert_eq!(r["status"], "pending", "首查必须 pending: {r}");
    assert_eq!(r["expires_at"], QR_EXPIRES);
    g.insert("login.qr_status.resp.pending.json".into(), resp);

    let (req, resp) = c.rpc("login.qr_status", json!({ "token": QR_TOKEN })).await;
    let r = parse_result(&resp);
    assert_eq!(r["status"], "ok", "再查必须收口: {r}");
    assert_eq!(r["name"], LOGIN_NAME);
    put(&mut g, "login.qr_status", req, resp);

    // 受信端应答(此时尚已登录,evict 收口的会话即 Bearer 面):空对象
    let (req, resp) = c
        .rpc(
            "approval.respond",
            json!({ "session_id": SESS_A, "allow": true }),
        )
        .await;
    assert_eq!(parse_result(&resp), json!({}));
    put(&mut g, "approval.respond", req, resp);

    // KV 透传:put → {"ok":true};get → {"value":…};未命 → typed not_found
    let (req, resp) = c
        .rpc("kv.put", json!({ "key": KV_KEY, "value": KV_VALUE }))
        .await;
    assert_eq!(parse_result(&resp), json!({ "ok": true }));
    put(&mut g, "kv.put", req, resp);

    let (req, resp) = c.rpc("kv.get", json!({ "key": KV_KEY })).await;
    let r = parse_result(&resp);
    assert_eq!(r, json!({ "value": KV_VALUE }), "KV 往返同值: {r}");
    put(&mut g, "kv.get", req, resp);

    let (_req, resp) = c.rpc("kv.get", json!({ "key": "不存在键" })).await;
    let (code, msg) = parse_error(&resp);
    assert_eq!(code, 1, "未命是 code 1");
    assert!(
        msg.contains("不存在键"),
        "未命文案带键名: {msg}"
    );
    g.insert("kv.get.resp.notfound.json".into(), resp);

    // 扫码授权闭环透传(Ruling R8;手机侧=已登录 daemon 持会话 Bearer 代打,
    // 桩断言 Bearer 头在场):claim 候选面拍平 → approve 状态投影;死码的
    // not_found/expired 两张变体帧同 capture(变体 req 不入库,主形态已锁)
    let (req, resp) = c.rpc("qr.claim", json!({ "token": QR_TOKEN })).await;
    let r = parse_result(&resp);
    assert_eq!(r["candidate_name"], QR_CANDIDATE_NAME, "候选名拍平投影: {r}");
    assert_eq!(r["candidate_platform"], "windows");
    assert_eq!(r["expires_at"], QR_EXPIRES);
    put(&mut g, "qr.claim", req, resp);

    let (_req, resp) = c.rpc("qr.claim", json!({ "token": QR_DEAD })).await;
    let (code, msg) = parse_error(&resp);
    assert_eq!(code, 1, "死码认领是 code 1");
    assert!(msg.contains("二维码已失效"), "死码文案: {msg}");
    g.insert("qr.claim.resp.expired.json".into(), resp);

    let (req, resp) = c
        .rpc("qr.approve", json!({ "token": QR_TOKEN, "allow": true }))
        .await;
    assert_eq!(parse_result(&resp), json!({ "status": "ok" }), "裁决受理投影 ok");
    put(&mut g, "qr.approve", req, resp);

    let (_req, resp) = c
        .rpc("qr.approve", json!({ "token": QR_DEAD, "allow": true }))
        .await;
    assert_eq!(
        parse_result(&resp),
        json!({ "status": "expired" }),
        "单次裁决重放/死码 → expired 投影(非 RPC 错)"
    );
    g.insert("qr.approve.resp.expired.json".into(), resp);

    // ---- 登出(幂等成功;此后的会话面回到未登录) ----
    let (req, resp) = c.rpc("login.logout", json!({})).await;
    assert_eq!(parse_result(&resp), json!({}));
    put(&mut g, "login.logout", req, resp);

    // ---- 事件帧 ----
    c.drain().await;
    // 真捕获:登录翻转(verify 成功即广播)与作业 "3" 建队帧(建卡同步
    // 发出,先于 start 应答;首帧即 queued 态)。
    let ev_login = take_event(&c.events, "event.login(logged_in=true)", |v| {
        v["method"] == "event.login" && v["params"]["logged_in"] == true
    });
    let ev_transfer = take_event(&c.events, "event.transfer(作业 3 建队帧)", |v| {
        v["method"] == "event.transfer" && v["params"]["id"] == "3"
    });
    {
        let v: Value = serde_json::from_slice(line_body(&ev_transfer)).unwrap();
        assert_eq!(v["params"]["state"], "queued", "首帧即建队态: {v}");
        assert_eq!(v["params"]["files_total"], 1);
        assert_eq!(v["params"]["bytes_total"], FIXTURE_SIZE as u64);
    }
    g.insert("event.login.json".into(), ev_login);
    g.insert("event.transfer.json".into(), ev_transfer);
    // 构造帧(发射点同形;event.devices 的目录列表取自真 devices RPC 的
    // 同一 build_snapshot 投影输出):
    g.insert(
        "event.online.json".into(),
        event_frame("event.online", json!({ "online": true })),
    );
    g.insert(
        "event.devices.json".into(),
        event_frame("event.devices", json!({ "devices": devices_list })),
    );
    g.insert(
        "event.notify.json".into(),
        event_frame(
            "event.notify",
            json!({
                "title": "MTDSS",
                "body": "«契约对端» 向 /mtdss/contract-inbox 写入 2 个文件",
            }),
        ),
    );
    // V3-1 事件(AuthNotice 路由面;桩无 /ws,fixture 不可达——按发射点
    // 同形构造,与 event.online/devices/notify 同一手法;三帧均在
    // login_v3p1 e2e 真捕获[event.devices_changed 走 admin-unbind 懒踢
    // 隔离锚:已知设备删行后快照消行只能来自通告路径,Presence 回退
    // 覆盖不到],形状漂移双保险):
    // - event.approval = hub.rs join_request 路由的五字段投影
    // - event.device_evicted / event.devices_changed = 空参数面
    g.insert(
        "event.approval.json".into(),
        event_frame(
            "event.approval",
            json!({
                "session_id": SESS_A,
                "candidate_name": "契约申请端",
                "candidate_platform": "windows",
                "code": "123456",
                "expires_at": SESS_EXPIRES,
            }),
        ),
    );
    g.insert(
        "event.device_evicted.json".into(),
        event_frame("event.device_evicted", json!({})),
    );
    g.insert(
        "event.devices_changed.json".into(),
        event_frame("event.devices_changed", json!({})),
    );

    // ---- 收摊(客户端句柄先弃,Windows 命名管道名随之释放) ----
    drop(c);
    daemon.shutdown().await.expect("daemon 收尾");
    stub.abort();
    g
}

// ---------- 比对 / 重生成 ----------

/// 常规跑:逐字节比对,缺失/漂移即红;`REGEN=1`:重写全部 golden。
fn compare_or_regen(goldens: BTreeMap<String, Vec<u8>>) {
    let regen = std::env::var("REGEN").ok().as_deref() == Some("1");
    if regen {
        std::fs::create_dir_all(GOLDEN_DIR).expect("建 golden 目录");
        for (name, bytes) in &goldens {
            std::fs::write(Path::new(GOLDEN_DIR).join(name), bytes)
                .unwrap_or_else(|e| panic!("写 golden {name}: {e}"));
        }
        eprintln!("REGEN: 已重写 {} 个 golden → {GOLDEN_DIR}", goldens.len());
        return;
    }
    let mut missing: Vec<String> = Vec::new();
    let mut drifted: Vec<String> = Vec::new();
    for (name, bytes) in &goldens {
        let path = Path::new(GOLDEN_DIR).join(name);
        let Ok(existing) = std::fs::read(&path) else {
            missing.push(name.clone());
            continue;
        };
        if &existing != bytes {
            drifted.push(name.clone());
            eprintln!(
                "[drift] {name}\n  旧: {}\n  新: {}",
                String::from_utf8_lossy(&existing),
                String::from_utf8_lossy(bytes),
            );
        }
    }
    assert!(
        missing.is_empty() && drifted.is_empty(),
        "IPC 契约 golden 不一致:缺失 {} 个 {missing:?},漂移 {} 个 {drifted:?}\n\
         重生成:REGEN=1 cargo test -p mtdss-daemon --test contract_golden\n\
         (REGEN 幂等:重跑无 git diff;生成后以普通模式复跑确认绿再提交)",
        missing.len(),
        drifted.len(),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn contract_golden_frames_match() {
    let goldens = tokio::time::timeout(SCENARIO_TIMEOUT, scenario())
        .await
        .expect("契约 golden 场景须在限时内完成");
    compare_or_regen(goldens);
}
