// release 构建转 Windows 子系统(无控制台窗口):安装器钩子 Exec、HKCU Run
// 自启动等 ShellExecute 形态拉起 mtdssd 时不再挂一个 cmd 窗口(M4b 冒烟
// §B 实测);debug 保留控制台便于开发观察日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! mtdssd:MTDSS 常驻守护进程入口。
//!
//! 解析旗标与 MTDSS_HOME,初始化日志(缺省 stderr;指定文件则按行 JSON
//! 落盘),起 `Daemon`(Windows 且未 --no-tray 时含系统托盘)后等待收尾
//! 信号:Ctrl-C 或托盘「退出」(经 quit_watch 与 Ctrl-C 同台 select),
//! 随后走完整 `daemon.shutdown`(任务收尾 + 启动锁删除)。

use clap::Parser;
use mtdss_daemon::{Daemon, DaemonOpts};
use std::path::{Path, PathBuf};
use tokio::sync::watch;

#[derive(Parser)]
#[command(name = "mtdssd", version, about = "MTDSS 常驻守护进程")]
struct Args {
    /// 接收目录;缺省按 settings.json 的 recv_dir,再缺省 {home}/inbox
    #[arg(long)]
    recv_dir: Option<PathBuf>,
    /// 日志文件;缺省输出到 stderr,指定则按行 JSON 落盘
    #[arg(long)]
    log_file: Option<PathBuf>,
    /// 不启用系统托盘(仅 Windows 有托盘面;非 Windows 本就无)
    #[arg(long)]
    no_tray: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // android(设计 §4.1,评审裁定⑥):先于一切启动动作挂父死信号——
    // android 上应用进程被杀时子进程不会自动连带死(reparent 到 init),
    // PR_SET_PDEATHSIG 保「App 亡引擎即亡」;正常退出路径由 Kotlin 侧
    // onDestroy destroy() 覆盖,本行兜异常杀。启动即设,竞窗最小。
    #[cfg(target_os = "android")]
    set_parent_death_signal();
    let args = Args::parse();
    init_tracing(args.log_file.as_deref())?;

    let home = mtdss_ipc::mtdss_home();
    // 接收目录优先级:--recv-dir 显式 > settings.recv_dir > {home}/inbox
    let settings = mtdss_daemon::settings::Settings::load(&home)?;
    let recv_dir = args
        .recv_dir
        .or_else(|| settings.recv_dir.as_deref().map(PathBuf::from))
        .unwrap_or_else(|| home.join("inbox"));

    let daemon = Daemon::start(
        &home,
        DaemonOpts {
            recv_dir,
            tray: !args.no_tray,
        },
    )
    .await?;
    tracing::info!(home = %home.display(), "守护进程已启动,等待收尾信号");

    let mut quit_rx = daemon.quit_watch();
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("收到 Ctrl-C,正在关闭");
        }
        _ = quit_requested(&mut quit_rx) => {
            tracing::info!("收到托盘「退出」请求,正在关闭");
        }
    }
    daemon.shutdown().await
}

/// 等待托盘「退出」请求;无托盘(None)即永久挂起(等于只等 Ctrl-C)。
/// 通道关闭(托盘线程异常消亡)按「托盘不可用」处理:守护进程继续服务,
/// 不因托盘异常连坐退出。
async fn quit_requested(quit_rx: &mut Option<watch::Receiver<bool>>) {
    match quit_rx {
        None => std::future::pending::<()>().await,
        Some(rx) => loop {
            if *rx.borrow_and_update() {
                return;
            }
            match rx.changed().await {
                Ok(()) => {}
                Err(_) => std::future::pending::<()>().await,
            }
        },
    }
}

/// 初始化日志:缺省 stderr(与 cli 同口径的 env-filter);指定文件则按行 JSON。
/// android 恒 stderr(mtdssd 由 App 子进程拉起,stderr 由宿主侧转 logcat,
/// 设计 §4.1 日志面)——--log-file 形参 android 不消费(Kotlin 侧不传)。
// android 下形参被恒 None 覆写不再读取,静默 unused-variables(同款口径
// 见 lib.rs start_inner)。
#[cfg_attr(target_os = "android", allow(unused_variables))]
fn init_tracing(log_file: Option<&Path>) -> anyhow::Result<()> {
    let filter =
        tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?);
    #[cfg(target_os = "android")]
    let log_file: Option<&Path> = None;
    match log_file {
        None => {
            tracing_subscriber::fmt().with_env_filter(filter).init();
        }
        Some(path) => {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .json()
                .with_writer(std::sync::Mutex::new(file))
                .init();
        }
    }
    Ok(())
}

/// android 专用(设计 §4.1 裁定⑥):挂父死信号,App 进程死亡即内核发
/// SIGKILL 收掉本进程。PR_SET_PDEATHSIG 仅在调用失败(选项/信号非法)时
/// 返回非零,两个常量都是合法值,启动即设无失败面——返回值不消费。
/// 注:PDEATHSIG 的触发点是「创建子进程的那个父线程」消亡,Kotlin 侧
/// 拉起须用与 App 同寿命的线程(主线程),避免引擎被误杀。
#[cfg(target_os = "android")]
fn set_parent_death_signal() {
    unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) };
}
