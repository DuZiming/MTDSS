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
fn init_tracing(log_file: Option<&Path>) -> anyhow::Result<()> {
    let filter =
        tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?);
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
