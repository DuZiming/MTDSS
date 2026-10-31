//! ios-bridge:iOS 进程内引擎桥(v2-P5 M1,设计 §5.1)。
//!
//! iOS 禁止 App 创建子进程 → 安卓「mtdssd 子进程 + UDS」不可平移;本 crate
//! 把引擎编成静态库链进主 App,在**进程内专属线程**跑引擎主循环,Swift 侧
//! 经同进程 Unix domain socket 以 JSON-RPC 消费引擎(ipc 契约的又一客户端,
//! 与 Kotlin 同款打法,golden 双端防漂移)。
//!
//! C ABI 三件套(签名钉死,T4 Swift 侧消费):
//! - [`mtdss_ios_start`]:起引擎线程(0 = 已起线程;非 0 = 错误码)。bin 的
//!   prelude(rustls ring provider 安装、init_tracing、settings/recv_dir 解析)
//!   迁入本入口——lib 化授权范围(设计 §5.1 builder 评审 #3;桌面 bin 行为
//!   不变,桌面仍走自己的 main.rs)。
//! - [`mtdss_ios_stop`]:优雅停引擎线程(watch 停车信号 → `Daemon::shutdown`
//!   全套收尾 → join)。
//! - [`mtdss_ios_ipc_path`]:按 ipc::socket_path 同源派生 UDS 名写出给定
//!   缓冲,返回所需长度(含 NUL;缓冲不足不写,调用方按返回值分配重调)。
//!
//! 线程与 panic 模型:引擎线程体经 [`run_engine_body`] 的 catch_unwind 包裹
//! ——引擎 panic 落 tracing error 后线程自然收尾,**不蔓延成进程 abort**
//! (iOS 无进程隔离,引擎 panic 不得杀 App);Swift 侧监督(hello 失败重启
//! 引擎线程、连续失败错误横幅)归 T4。
//!
//! 授权根(v2-P5 T2,spec §3):iOS 形态注入 `authorized_roots` = 收件箱
//! (默认收件目录,恒在)+ 信任目录(T10 书签解析结果经此面扩注入);引擎
//! 的 ctrl 浏览与发送源读取在 daemon 侧收口,集外路径一律拒。
//!
//! `no_tray`/无更新面:iOS 无托盘(桌面专属模块已 cfg 剔除)、无自更新
//! (商店/TestFlight 接管;非 Windows 的更新面本就是单元占位)。

use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use mtdss_daemon::{Daemon, DaemonOpts};

/// start 错误码:入参非法(null / 非 UTF-8)。
pub const ERR_START_ARGS: c_int = 1;
/// start 错误码:引擎已在运行(未 stop 重复 start)。
pub const ERR_START_RUNNING: c_int = 2;
/// start 错误码:引擎启动失败(线程创建失败 / daemon 装配失败;细节留
/// tracing)。
pub const ERR_START_FAILED: c_int = 3;
/// stop 返回码:引擎未在运行。
pub const ERR_STOP_NOT_RUNNING: c_int = 1;

/// ipc_path 负值错误码:home 入参 null。
const ERR_IPC_PATH_NULL: c_int = -1;
/// ipc_path 负值错误码:home 非 UTF-8。
const ERR_IPC_PATH_UTF8: c_int = -2;

/// 引擎单例槽:stop 信号 + 引擎线程 join 句柄 + 生命标志。桥的运行时形态
/// 即单引擎(App 进程内一个引擎线程;Swift 监督重启 = stop 后再 start)。
struct EngineHandle {
    /// 停车信号:stop 置 true,引擎线程在 `Daemon::start` 成功后等它翻转,
    /// 翻转即走完整 `Daemon::shutdown`。
    stop: tokio::sync::watch::Sender<bool>,
    /// 引擎线程 join 句柄(stop 语义 = 等收尾完毕再返回)。
    thread: Option<std::thread::JoinHandle<()>>,
    /// 引擎真的起来了(`Daemon::start` 返回 Ok);stop 据此区分「优雅停了
    /// 一台在跑的引擎(0)」与「停了一台没起来的(非 0)」。
    started: Arc<AtomicBool>,
    /// 引擎线程已终局(start 失败或 panic 收口,线程即将退出);start 据
    /// 此允许在死槽上重启(Swift 监督的 restart 路径)。
    dead: Arc<AtomicBool>,
}

static ENGINE: OnceLock<Mutex<Option<EngineHandle>>> = OnceLock::new();
/// init_tracing 进程级一次(重复 init 会 panic;App 进程内桥入口只装一次)。
static TRACING: OnceLock<()> = OnceLock::new();

fn engine_slot() -> &'static Mutex<Option<EngineHandle>> {
    ENGINE.get_or_init(|| Mutex::new(None))
}

/// 起引擎线程(home = MTDSS 数据目录;接收目录按 bin 同款优先级:settings
/// → `{home}/inbox`)。0 = 已起线程;非 0 = [`ERR_START_ARGS`] /
/// [`ERR_START_RUNNING`] / [`ERR_START_FAILED`]。
///
/// 就绪语义:返回即线程已起,IPC 就绪以连上 UDS 为准(Swift 侧轮询 hello,
/// 与桌面 UI 同口径;监督面归 T4)。
///
/// # Safety
/// `home_path` 必须指向合法 NUL 结尾 UTF-8 C 字符串(调用方持有)。
#[no_mangle]
pub extern "C" fn mtdss_ios_start(home_path: *const c_char) -> c_int {
    let home = match cstr_to_path(home_path) {
        Some(p) => p,
        None => return ERR_START_ARGS,
    };
    engine_prelude();
    // bin 同款接收目录解析(--recv-dir 面无桥对应,缺省优先级一致)。
    let settings = match mtdss_daemon::settings::Settings::load(&home) {
        Ok(s) => s,
        Err(err) => {
            tracing::error!(%err, home = %home.display(), "settings.json 损坏,引擎拒绝启动");
            return ERR_START_FAILED;
        }
    };
    let recv_dir = settings
        .recv_dir
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("inbox"));
    let opts = DaemonOpts {
        recv_dir: recv_dir.clone(),
        tray: false,
        // iOS 授权形态(spec §3):收件箱恒在(默认收件目录);信任目录由
        // T10 书签解析结果扩注入本集(现无书签面,先注入收件箱)。
        authorized_roots: Some(vec![recv_dir]),
    };
    let mut slot = engine_slot().lock().unwrap();
    if let Some(h) = slot.as_ref() {
        if !h.dead.load(Ordering::SeqCst) {
            return ERR_START_RUNNING;
        }
    }
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let started = Arc::new(AtomicBool::new(false));
    let dead = Arc::new(AtomicBool::new(false));
    let thread = match std::thread::Builder::new()
        .name("mtdss-ios-engine".into())
        .spawn({
            let started = started.clone();
            let dead = dead.clone();
            move || engine_main(home, opts, stop_rx, started, dead)
        }) {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(%err, "引擎线程创建失败");
            return ERR_START_FAILED;
        }
    };
    *slot = Some(EngineHandle {
        stop: stop_tx,
        thread: Some(thread),
        started,
        dead,
    });
    0
}

/// 优雅停引擎线程:发停车信号,等 `Daemon::shutdown` 收尾与线程 join 完毕
/// 再返回。0 = 已收尾;非 0 = [`ERR_STOP_NOT_RUNNING`]。
#[no_mangle]
pub extern "C" fn mtdss_ios_stop() -> c_int {
    // 先摘槽再 join:锁不得跨 join(引擎线程终局不回锁,无死锁面;此处
    // 摘除后 start 立即可重启,Semantics 与桌面「退出即让位」一致)。
    let handle = match engine_slot().lock().unwrap().take() {
        Some(h) => h,
        None => return ERR_STOP_NOT_RUNNING,
    };
    let was_started = handle.started.load(Ordering::SeqCst);
    let _ = handle.stop.send(true);
    if let Some(t) = handle.thread {
        // join 失败(线程已 panic 收口)不反噬:进程继续,错误已在线程内
        // tracing 留痕。
        let _ = t.join();
    }
    if was_started {
        0
    } else {
        ERR_STOP_NOT_RUNNING
    }
}

/// 派生 UDS 名(与 daemon/客户端同源 `ipc::socket_path`)写入 `out`,返回
/// **所需长度**(含 NUL)。缓冲够长即写出并返回所需长度;`out` 为 null 或
/// 不够长不写,仍返回所需长度(调用方按值分配后重调)。错误(负值):
/// [`ERR_IPC_PATH_NULL`] = 入参 null;[`ERR_IPC_PATH_UTF8`] = 非 UTF-8。
///
/// # Safety
/// `home_path` 必须指向合法 NUL 结尾 UTF-8 C 字符串;`out`(非 null 时)
/// 必须可写 `out_len` 字节。
#[no_mangle]
pub extern "C" fn mtdss_ios_ipc_path(
    home_path: *const c_char,
    out: *mut c_char,
    out_len: usize,
) -> c_int {
    let home = match cstr_to_path(home_path) {
        Some(p) => p,
        None => return ERR_IPC_PATH_NULL,
    };
    let path = mtdss_ipc::socket_path(&home);
    let bytes = path.as_bytes();
    let needed = bytes.len() + 1; // 含 NUL
    if needed > c_int::MAX as usize {
        // 路径长度物理不可能越 i32;防御留痕不写。
        tracing::error!(len = needed, "ipc path 越界,拒绝写出");
        return ERR_IPC_PATH_UTF8;
    }
    if !out.is_null() && out_len >= needed {
        // NUL 结尾的 UTF-8 字节串:CStr 语义逐字节拷入 + 终止符。
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, out, bytes.len());
            *out.add(bytes.len()) = 0;
        }
    }
    needed as c_int
}

/// bin prelude 迁入(M1 lib 化授权面,设计 §5.1):进程级 rustls ring
/// 默认 provider 幂等兜底 + tracing 一次装配(缺省 stderr——iOS 侧由
/// 宿主转 os_log,与 android stderr→logcat 同款日志面;--log-file 形态
/// 无桥对应,iOS 不落文件)。已装配时静默让路,幂等无害。
fn engine_prelude() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    if TRACING.set(()).is_ok() {
        let filter = tracing_subscriber::EnvFilter::from_default_env()
            .add_directive("info".parse().expect("info 指令合法"));
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
}

/// 引擎线程主循环:进程内 runtime 上起 `Daemon::start`,成功后等停车信号,
/// 翻转即完整收尾。panic 由 [`run_engine_body`] 收口(不杀 App);无论
/// 正常/异常终局,`dead` 置位放行下一次 start(Swift 监督重启路径)。
fn engine_main(
    home: PathBuf,
    opts: DaemonOpts,
    stop_rx: tokio::sync::watch::Receiver<bool>,
    started: Arc<AtomicBool>,
    dead: Arc<AtomicBool>,
) {
    run_engine_body(std::panic::AssertUnwindSafe(|| {
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(err) => {
                tracing::error!(%err, "引擎 runtime 创建失败");
                return;
            }
        };
        rt.block_on(async move {
            let daemon = match Daemon::start(&home, opts).await {
                Ok(d) => d,
                Err(err) => {
                    tracing::error!(%err, home = %home.display(), "iOS 引擎启动失败");
                    return;
                }
            };
            started.store(true, Ordering::SeqCst);
            tracing::info!(home = %home.display(), "iOS 引擎已启动(进程内线程)");
            // 停车等待:send 端 drop(桥整体卸载)也按停处理,引擎不留孤儿。
            let mut rx = stop_rx;
            let _ = rx.changed().await;
            if let Err(err) = daemon.shutdown().await {
                tracing::warn!(%err, "iOS 引擎收尾异常");
            }
        });
        // rt 随闭包 drop:worker 收干,线程自然终局。
    }));
    dead.store(true, Ordering::SeqCst);
}

/// 引擎线程体的 catch_unwind 包裹(简报测试②的桥内 API):panic 落
/// tracing error 后正常返回,线程自然收尾——**不蔓延**(iOS 无进程隔离,
/// 引擎 panic 不得杀 App;Swift 侧监督经 hello 失败检测并重启,T4)。
pub(crate) fn run_engine_body(f: std::panic::AssertUnwindSafe<impl FnOnce()>) {
    if let Err(panic) = std::panic::catch_unwind(f) {
        let msg = if let Some(s) = panic.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = panic.downcast_ref::<String>() {
            s.clone()
        } else {
            "非字符串 panic 载荷".to_string()
        };
        tracing::error!(panic = %msg, "iOS 引擎线程 panic(已收口,不杀进程;待 Swift 监督重启)");
    }
}

/// C 字符串 → PathBuf:null / 非 UTF-8 均按入参非法返回 None。
fn cstr_to_path(p: *const c_char) -> Option<PathBuf> {
    if p.is_null() {
        return None;
    }
    // Safety:调用方契约(NUL 结尾合法 C 字符串);from_ptr 只借不夺。
    let s = unsafe { CStr::from_ptr(p) }.to_str().ok()?;
    Some(PathBuf::from(s))
}

// 编译期口径检查(零运行时成本):签名面钉死给 T4 消费,防手滑漂移。
const _: () = {
    assert!(std::mem::size_of::<c_int>() == 4, "C int 契约按 32 位钉");
};

#[cfg(test)]
mod engine_body_tests {
    use super::*;
    use std::ffi::CString;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// 简 Step 1 ②:注入 panic 的子例程不蔓延——run_engine_body 正常返回
    /// (panic 在包裹内收口),且对正常体原样放行。
    #[test]
    fn engine_panic_is_contained() {
        run_engine_body(std::panic::AssertUnwindSafe(|| {
            panic!("注入的引擎 panic(测试②)")
        }));
        // 走到这里 = panic 未蔓延出 run_engine_body;再验正常体完整执行。
        let ok = Arc::new(AtomicBool::new(false));
        let flag = ok.clone();
        run_engine_body(std::panic::AssertUnwindSafe(move || {
            flag.store(true, Ordering::SeqCst);
        }));
        assert!(ok.load(Ordering::SeqCst), "正常体应完整执行");
    }

    /// cstr_to_path 入参面:null 与非 UTF-8 均拒绝(桥内纯函数直测)。
    #[test]
    fn cstr_to_path_rejects_null_and_non_utf8() {
        assert!(cstr_to_path(std::ptr::null()).is_none());
        let bytes = [0x68u8, 0x69, 0xFF, 0x00]; // "hi" + 非 UTF8 字节 + NUL
        let p = bytes.as_ptr() as *const c_char;
        assert!(cstr_to_path(p).is_none());
        let ok = CString::new("/tmp/x").unwrap();
        assert_eq!(cstr_to_path(ok.as_ptr()), Some(PathBuf::from("/tmp/x")));
    }
}
