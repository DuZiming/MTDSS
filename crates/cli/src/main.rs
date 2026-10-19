//! mtdss 命令行入口:纯 IPC 客户端。
//!
//! clap 只做解析;命令体全部在 `mtdss_cli::client`(连接/自动拉起守护进程
//! 与 RPC 命令层,与 ipc_cli.rs 共用)。登录的交互段(回显 dev 码、stdin
//! 读码)留在本文件——库层不碰终端。旧的全局 `--ca` 旗标与 `register`
//! 子命令已死:ca 是 login 的参数,经 IPC 持久化进会话。
//!
//! 退出码:0 成功;2 前置条件不满足(未登录/找不到 daemon,见 client 的
//! CliError 约定);其余失败 1(业务失败报告全文随 stderr 出)。

use clap::{Parser, Subcommand};
use mtdss_cli::client::{self, CliError, DaemonClient};
use std::io::Write as _;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "mtdss",
    version,
    about = "Mutual-TOFU device file transfer(经本机守护进程)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 登录账号:请求验证码 → 终端回显 dev 码 → 输入验证码完成验证
    Login {
        /// 信令服务器地址(https://host[:port])
        #[arg(long)]
        server: String,
        /// 账号邮箱
        #[arg(long)]
        email: String,
        /// 自签 CA 证书(PEM);持久化到会话
        #[arg(long)]
        ca: Option<PathBuf>,
        /// 设备名(缺省用本机主机名)
        #[arg(long)]
        name: Option<String>,
    },
    /// 登出本机设备
    Logout,
    /// 列出账号设备
    Devices,
    /// 显示本机守护进程状态
    Status,
    /// 发送文件或目录到设备
    Send {
        /// 文件或目录(可多个;目录整树在对端还原)
        files: Vec<PathBuf>,
        /// 目标设备 id
        #[arg(long)]
        to: String,
    },
    /// 设置常驻接收目录(守护进程收件落盘位置)
    Recv {
        /// 接收目录
        #[arg(long)]
        out: PathBuf,
    },
    /// 浏览远端设备目录
    Ls {
        /// 目标设备(device_id 或设备名;名字重名时须用 id)
        device: String,
        /// 远端路径(缺省 = 根卷)
        path: Option<String>,
    },
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("info".parse().expect("静态指令必可解析")),
        )
        .init();
    let cli = Cli::parse();
    // 设备数据目录在入口构造一次后显式下传;daemon 侧同口径派生,两侧不歧义
    let home = mtdss_ipc::mtdss_home();
    match run(&home, cli.cmd).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            std::process::ExitCode::from(e.code.clamp(0, 255) as u8)
        }
    }
}

/// 命令分发:连接(或自动拉起)守护进程 → 登录守门(login/status 豁免)
/// → 执行。
async fn run(home: &Path, cmd: Cmd) -> Result<(), CliError> {
    let c = DaemonClient::connect_or_spawn(home).await?;
    let exempt = matches!(cmd, Cmd::Login { .. } | Cmd::Status);
    client::ensure_logged_in(&c, exempt)?;

    match cmd {
        Cmd::Login { server, email, ca, name } => {
            // dev 模式服务器把验证码随 request_code 响应回显:打印给终端,
            // 再从 stdin 读回(harness 场景抓码回填 FIFO,同旧 register 型)
            let code = client::login_request_code(&c, &server, &email, ca.as_deref()).await?;
            println!("dev code: {code}");
            print!("enter code: ");
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            std::io::stdin()
                .read_line(&mut line)
                .map_err(|e| CliError { code: 1, message: format!("读入验证码失败: {e}") })?;
            let r = client::login_verify(&c, &email, line.trim(), name.as_deref()).await?;
            println!("已登录:device_id={}", r["device_id"].as_str().unwrap_or("?"));
            if let Some(n) = r["name"].as_str() {
                println!("设备名:{n}");
            }
        }
        Cmd::Logout => client::login_logout(&c).await?,
        Cmd::Devices => {
            // 行格式:<device_id>\t<name>\t<platform>\tonline=<bool>
            // (nat-matrix presence 轮询按行 grep;字段全平铺,无二次解析)
            let devs = client::devices_cmd(&c).await?;
            for d in devs["devices"].as_array().cloned().unwrap_or_default() {
                println!(
                    "{}\t{}\t{}\tonline={}",
                    d["device_id"].as_str().unwrap_or("?"),
                    d["name"].as_str().unwrap_or(""),
                    d["platform"].as_str().unwrap_or(""),
                    d["online"].as_bool().unwrap_or(false)
                );
            }
        }
        Cmd::Status => {
            let st = client::status_cmd(&c).await?;
            println!("logged_in={}", st["logged_in"].as_bool().unwrap_or(false));
            println!("online={}", st["online"].as_bool().unwrap_or(false));
            if let Some(s) = st["server"].as_str() {
                println!("server={s}");
            }
            println!("device_id={}", st["device"]["device_id"].as_str().unwrap_or("-"));
            println!("recv_dir={}", st["recv_dir"].as_str().unwrap_or(""));
        }
        Cmd::Send { files, to } => client::send_cmd(&c, &files, &to).await?,
        Cmd::Recv { out } => {
            client::set_recv_dir_cmd(&c, &out).await?;
            println!("接收目录已设置:{}", out.display());
        }
        Cmd::Ls { device, path } => {
            // devices 快照 → 定位(id 精确优先,名字须唯一)→ list → 行格式
            // 打印(format_ls 自带行尾换行,空目录输出为空)
            let devs = client::devices_cmd(&c).await?;
            let dev_id = client::resolve_device(
                devs["devices"].as_array().map(Vec::as_slice).unwrap_or(&[]),
                &device,
            )?;
            let listing = client::ls_cmd(&c, &dev_id, path.as_deref().unwrap_or("")).await?;
            print!("{}", client::format_ls(&listing));
        }
    }
    Ok(())
}
