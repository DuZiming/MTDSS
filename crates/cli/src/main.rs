//! mtdss 命令行入口：仅 clap 分发，逻辑在 `mtdss_cli`（lib）。

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "mtdss", version, about = "Mutual-TOFU device file transfer")]
struct Cli {
    /// Trust this CA cert (PEM) for https/wss signaling; register 持久化到 session
    #[arg(long, global = true)]
    ca: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Register this device with an account (email + code)
    Register {
        #[arg(long)]
        server: String,
        #[arg(long)]
        email: String,
        #[arg(long, default_value = "cli-device")]
        name: String,
    },
    /// List devices on the account
    Devices,
    /// Send one file to a device
    Send {
        file: PathBuf,
        #[arg(long)]
        to: String,
    },
    /// Receive files into a directory (waits for offers)
    Recv {
        #[arg(long, default_value = ".")]
        out: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?),
        )
        .init();
    let cli = Cli::parse();
    // 设备数据目录在入口构造一次后显式下传（flow 不再读进程环境），
    // 便于测试在同一进程内并发驱动多个互不干扰的设备目录。
    let home = mtdss_cli::session::mtdss_home();
    match cli.cmd {
        Cmd::Register {
            server,
            email,
            name,
        } => mtdss_cli::flow::register(&home, &server, &email, name, cli.ca.as_deref()).await,
        Cmd::Devices => {
            // --ca 全局旗标：加载会话后覆盖,再传入
            let mut s = mtdss_cli::session::Session::load(&home)?;
            if let Some(c) = cli.ca.clone() {
                s.ca = Some(c);
            }
            mtdss_cli::flow::devices_cmd(&s).await
        }
        Cmd::Send { file, to } => {
            let mut s = mtdss_cli::session::Session::load(&home)?;
            if let Some(c) = cli.ca.clone() {
                s.ca = Some(c);
            }
            // Task 7 会做完整的多路径 CLI 接线;此处先按单文件切片适配新签名
            mtdss_cli::flow::run_send(&s, &home, std::slice::from_ref(&file), &to, &|m| {
                println!("{m}")
            })
            .await
        }
        Cmd::Recv { out } => {
            let mut s = mtdss_cli::session::Session::load(&home)?;
            if let Some(c) = cli.ca.clone() {
                s.ca = Some(c);
            }
            let paths = mtdss_cli::flow::run_recv(&s, &home, &out).await?;
            if paths.is_empty() {
                // 全 SKIP 的重发轮:对端已全部持有,无事可做
                println!("对端已全部持有,无事可做");
            } else {
                for p in &paths {
                    println!("received: {}", p.display());
                }
            }
            Ok(())
        }
    }
}
