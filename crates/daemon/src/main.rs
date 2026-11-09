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
    /// 接收目录;缺省按 settings.json 的 recv_dir,再缺省 文档\MTDSS
    /// (V3-2 §4;非 Windows/文档目录缺失回退 {home}/inbox)
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
    // 进程级 rustls 默认 provider 兜底(2026-10-30 域名切换公共 CA 首踩
    // provider 歧义 panic):workspace 特性统一后 ring 与 aws-lc-rs 并存,
    // 任何残余的惰性 rustls builder(默认路径自动检测 process 默认
    // provider)都会歧义 panic——信令 wss 的 ca=None 路径已在 core 显式
    // ring,此处再在一切任务 spawn 之前安装默认,兜深处的隐性 builder。
    // 已安装时 Err 静默让路,幂等无害。
    let _ = rustls::crypto::ring::default_provider().install_default();
    // android(设计 §4.1,评审裁定⑥):先于一切启动动作挂父死信号——
    // android 上应用进程被杀时子进程不会自动连带死(reparent 到 init),
    // PR_SET_PDEATHSIG 保「App 亡引擎即亡」;正常退出路径由 Kotlin 侧
    // onDestroy destroy() 覆盖,本行兜异常杀。启动即设,竞窗最小。
    #[cfg(target_os = "android")]
    set_parent_death_signal();
    let args = Args::parse();
    init_tracing(args.log_file.as_deref())?;

    let home = mtdss_ipc::mtdss_home();
    // 接收目录优先级(V3-2 §4,经 lib 的唯一装配面):--recv-dir 显式
    // > settings.recv_dir > 文档\MTDSS(Windows) > {home}/inbox
    let settings = mtdss_daemon::settings::Settings::load(&home)?;
    #[cfg(windows)]
    let documents = dirs::document_dir();
    #[cfg(not(windows))]
    let documents = None;
    let recv_dir = mtdss_daemon::resolve_recv_dir(
        args.recv_dir,
        settings.recv_dir.as_deref().map(PathBuf::from),
        documents,
        &home,
    );
    // V3-3 返工 R2(spec §13.3):启动即建收件目录(best-effort)——resolve
    // 只算路径不落盘,全新安装(如 Windows 默认 文档\MTDSS)路径不存在,
    // 收件箱浏览直接 NOT_FOUND;此处逐级建齐兜底,失败仅告警不阻启(真实
    // 不可写时后续接收面自会按错误上报)。
    if let Err(e) = std::fs::create_dir_all(&recv_dir) {
        tracing::warn!("创建收件目录 {} 失败(不阻启): {e}", recv_dir.display());
    }

    // V3-4 §5-2:可选信任目录模式(settings.json 信任段;默认关 = 直通,
    // 桌面行为零回归)。开 = 启动授权根 = 收件箱(生效接收目录)+ 用户
    // 所选目录(trusted_roots 顺序保留;非法路径由守卫构造期丢弃
    // fail-closed);关/缺省 = None 直通。运行时切换免重启:ui 经
    // roots.set 热注册覆盖本初值(T1 IPC op,守卫可换)。android bin 的
    // App 沙箱 home 无 settings.json,恒取缺省 = 与既有 None 注入同形
    // (App 随后经 roots.set 同步授权集,语义不变)。
    let authorized_roots = if settings.trust_mode {
        let mut roots = vec![recv_dir.clone()];
        roots.extend(settings.trusted_roots.iter().map(PathBuf::from));
        Some(roots)
    } else {
        None
    };

    let daemon = Daemon::start(
        &home,
        DaemonOpts {
            recv_dir,
            tray: !args.no_tray,
            authorized_roots,
            inbox_staging: None,
            // 收件箱发布面随编译目标(v2-P5 T5b Ruling J):android/iOS 哨兵
            // 路由 InboxDestIo;桌面哨兵回退默认收件目录。
            inbox_publish: cfg!(any(target_os = "android", target_os = "ios")),
            // 照片桥(R7/V3-4 T3):iOS/android 引擎进程注入(bin 形态恒关);
            // android 权限即数据面,归 App 应答器答(T14 接线)。
            photo_serve: cfg!(any(target_os = "android", target_os = "ios")),
            // ctrl 直连调优 env 推导(V3-5 测试注入面):MTDSS_TEST_CTRL_
            // KEEPALIVE_MS / MTDSS_TEST_CTRL_IDLE_MS;缺省 None = 默认 15s
            // 心跳(非法值 warn 保持默认,沿 apply_test_env 房风)。
            ctrl_tuning: mtdss_daemon::flow::ctrl_tuning_from_env(|k| std::env::var(k).ok()),
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
