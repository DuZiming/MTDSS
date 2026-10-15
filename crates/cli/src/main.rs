//! mtdss 命令行入口：仅 clap 分发，逻辑在 `mtdss_cli`（lib）。

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "mtdss", version, about = "Mutual-TOFU device file transfer")]
struct Cli {
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
    match cli.cmd {
        Cmd::Register { server, email, name } => {
            mtdss_cli::flow::register(&server, &email, &name).await
        }
        Cmd::Devices => mtdss_cli::flow::devices_cmd().await,
        Cmd::Send { file, to } => {
            let s = mtdss_cli::session::Session::load()?;
            mtdss_cli::flow::run_send(&s, &file, &to, &|m| println!("{m}")).await
        }
        Cmd::Recv { out } => {
            let s = mtdss_cli::session::Session::load()?;
            let p = mtdss_cli::flow::run_recv(&s, &out).await?;
            println!("received: {}", p.display());
            Ok(())
        }
    }
}
