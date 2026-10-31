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

use std::os::raw::{c_char, c_int};

/// start 错误码:入参非法(null / 非 UTF-8)。
pub const ERR_START_ARGS: c_int = 1;
/// start 错误码:引擎已在运行(未 stop 重复 start)。
pub const ERR_START_RUNNING: c_int = 2;
/// start 错误码:引擎启动失败(线程创建失败 / daemon 装配失败;细节留
/// tracing)。
pub const ERR_START_FAILED: c_int = 3;
/// stop 返回码:引擎未在运行。
pub const ERR_STOP_NOT_RUNNING: c_int = 1;

/// 起引擎线程(home = MTDSS 数据目录;接收目录按 bin 同款优先级:settings
/// → `{home}/inbox`)。0 = 已起线程;非 0 = [`ERR_START_ARGS`] /
/// [`ERR_START_RUNNING`] / [`ERR_START_FAILED`]。
///
/// # Safety
/// `home_path` 必须指向合法 NUL 结尾 UTF-8 C 字符串(调用方持有)。
#[no_mangle]
pub extern "C" fn mtdss_ios_start(home_path: *const c_char) -> c_int {
    if home_path.is_null() {
        return ERR_START_ARGS;
    }
    // TDD 红桩:实现于绿步(测试 ①/④ 在本形态下必红)。
    ERR_START_FAILED
}

/// 优雅停引擎线程:发停车信号,等 `Daemon::shutdown` 收尾与线程 join 完毕
/// 再返回。0 = 已收尾;非 0 = [`ERR_STOP_NOT_RUNNING`]。
#[no_mangle]
pub extern "C" fn mtdss_ios_stop() -> c_int {
    // TDD 红桩:实现于绿步。
    ERR_STOP_NOT_RUNNING
}

/// 派生 UDS 名(与 daemon/客户端同源 `ipc::socket_path`)写入 `out`,返回
/// **所需长度**(含 NUL)。缓冲够长即写出并返回所需长度;`out` 为 null 或
/// 不够长不写,仍返回所需长度(调用方按值分配后重调)。错误(负值):
/// [`ERR_START_ARGS`] 同码 -1 = 入参 null;-2 = 非 UTF-8。
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
    let _ = (home_path, out, out_len);
    // TDD 红桩:实现于绿步(测试 ③ 在本形态下必红)。
    -3
}

/// 引擎线程体的 catch_unwind 包裹(简报测试②的桥内 API):panic 落
/// tracing error 后正常返回,线程自然收尾——**不蔓延**(iOS 无进程隔离,
/// 引擎 panic 不得杀 App;Swift 侧监督经 hello 失败检测并重启,T4)。
// TDD 红步:生产接线(engine_main)在绿步落地,先挂 allow 保 lint 净。
#[allow(dead_code)]
pub(crate) fn run_engine_body(f: impl FnOnce() + std::panic::UnwindSafe) {
    // TDD 红桩:无包裹直跑(测试 ② 注入 panic 在本形态下蔓延,必红)。
    f()
}

#[cfg(test)]
mod engine_body_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// 简 Step 1 ②:注入 panic 的子例程不蔓延——run_engine_body 正常返回
    /// (panic 在包裹内收口),且对正常体原样放行。
    #[test]
    fn engine_panic_is_contained() {
        run_engine_body(|| panic!("注入的引擎 panic(测试②)"));
        // 走到这里 = panic 未蔓延出 run_engine_body;再验正常体完整执行。
        let ok = Arc::new(AtomicBool::new(false));
        let flag = ok.clone();
        run_engine_body(move || {
            flag.store(true, Ordering::SeqCst);
        });
        assert!(ok.load(Ordering::SeqCst), "正常体应完整执行");
    }
}
