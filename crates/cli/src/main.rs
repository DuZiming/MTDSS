//! mtdss 命令行入口:纯 IPC 客户端。
//!
//! clap 只做解析;命令体全部在 `mtdss_cli::client`(连接/自动拉起守护进程
//! 与 RPC 命令层,与 ipc_cli.rs 共用)。登录与冲突问询的交互段(回显 dev
//! 码、stdin 读码/读冲突应答)留在本文件——库层只出纯数据
//! (conflict_prompt_data/parse_conflict_answer),不碰终端。旧的全局
//! `--ca` 旗标与 `register` 子命令已死:ca 是 login 的参数,经 IPC 持久化
//! 进会话。
//!
//! 退出码:0 成功;2 前置条件不满足(未登录/找不到 daemon,见 client 的
//! CliError 约定);其余失败 1(业务失败报告全文随 stderr 出)。put/get/cp
//! 阻塞至 transfer.wait 终态,退出码按 state(done=0,其余 1)。

use clap::{Parser, Subcommand, ValueEnum};
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

/// put/get 的冲突策略(--conflict;kebab-case 由 clap ValueEnum 解析)。
#[derive(ValueEnum, Clone, Copy)]
enum ConflictMode {
    /// 有冲突时交互问询(默认):全部跳过/全部覆盖/取消。
    Ask,
    /// 全部跳过:同名条目不落盘。
    Skip,
    /// 全部覆盖:同名条目清障后原位落盘。
    Overwrite,
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
    /// 上传到设备(scp 形目标;作业化传输,阻塞至终态)
    Put {
        /// 目标 `<设备>:<远端绝对目录>`(设备为 id 或名;名重名须用 id)
        target: String,
        /// 冲突策略
        #[arg(long, default_value = "ask")]
        conflict: ConflictMode,
        /// 本地文件或目录(可多个;目录整树在远端还原)
        files: Vec<PathBuf>,
    },
    /// 从设备拉取(作业化传输,阻塞至终态)
    Get {
        /// 远端源 `<设备>:<远端绝对路径>`(可多个;须同一台设备)
        #[arg(required = true)]
        targets: Vec<String>,
        /// 本机落盘目录(绝对形态)
        #[arg(long)]
        to: PathBuf,
        /// 冲突策略
        #[arg(long, default_value = "ask")]
        conflict: ConflictMode,
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
    /// 设备间复制(cp;四态分发,阻塞至终态)
    Cp {
        /// 源 `<设备>:<远端绝对路径>` 或裸本机路径(可多个;须同一侧同一台)
        #[arg(required = true)]
        sources: Vec<String>,
        /// 目标 `<设备>:<远端绝对目录>` 或本机目录
        #[arg(long)]
        to: String,
        /// 冲突策略(同机复制糖不适用:接收端自动改名)
        #[arg(long, default_value = "ask")]
        conflict: ConflictMode,
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
        Cmd::Put { target, conflict, files } => {
            // devices 快照 → 定位 → 冲突预扫(ask 有冲突即问询)→ start →
            // wait 至终态,退出码按 state
            let (dev_name, dir) = client::parse_scp_target(&target)?;
            let dev_id = lookup_device(&c, &dev_name).await?;
            let conflicts = client::scan_put_conflicts(&c, &dev_id, &dir, &files).await?;
            let (skip, overwrite) = resolve_conflicts(&conflicts, conflict)?;
            let id = client::start_put(&c, &files, &dev_id, &dir, &skip, &overwrite).await?;
            finish_transfer(client::wait_job(&c, &id).await?)?;
        }
        Cmd::Get { targets, to, conflict } => {
            // 逐目标解析设备段(全部须同一台)→ 定位 → 冲突预扫 → start → wait
            let mut dev_name: Option<String> = None;
            let mut remote_paths: Vec<String> = Vec::with_capacity(targets.len());
            for t in &targets {
                let (d, p) = client::parse_scp_target(t)?;
                match &dev_name {
                    None => dev_name = Some(d),
                    Some(prev) if *prev == d => {}
                    Some(prev) => {
                        return Err(CliError {
                            code: 1,
                            message: format!("get 的多个目标须同一台设备:「{prev}」与「{d}」不符"),
                        });
                    }
                }
                remote_paths.push(p);
            }
            let dev_name = dev_name.expect("targets 非空由 clap required 保证");
            let dev_id = lookup_device(&c, &dev_name).await?;
            let conflicts = client::scan_get_conflicts(&c, &dev_id, &remote_paths, &to).await?;
            let (skip, overwrite) = resolve_conflicts(&conflicts, conflict)?;
            let id = client::start_get(&c, &dev_id, &remote_paths, &to, &skip, &overwrite).await?;
            finish_transfer(client::wait_job(&c, &id).await?)?;
        }
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
        Cmd::Cp { sources, to, conflict } => cp_cmd(&c, sources, to, conflict).await?,
    }
    Ok(())
}

/// 设备定位(devices 快照 + resolve_device;put/get 共用)。
async fn lookup_device(c: &DaemonClient, name_or_id: &str) -> Result<String, CliError> {
    let devs = client::devices_cmd(c).await?;
    client::resolve_device(
        devs["devices"].as_array().map(Vec::as_slice).unwrap_or(&[]),
        name_or_id,
    )
}

/// cp 的四态分发(Task 8,计划钉死):`--to` 本机目录形态 = download 糖
/// (远端→本机)、源全本机 = upload 糖(本机→远端)、源设备 == 目标设备 =
/// `copy` RPC 糖(同机复制不经传输面)、其余 = 真 remote_copy(设备间直
/// 传,数据不经本机落地)。冲突预扫顶层粒度(V1 已知限制),问询/策略与
/// put/get 同一交互面;同机糖无策略参数(接收端 unique_target 自动改名),
/// 命中冲突只提示不打断。
async fn cp_cmd(
    c: &DaemonClient,
    sources: Vec<String>,
    to: String,
    conflict: ConflictMode,
) -> Result<(), CliError> {
    let (to_dev, to_dir) = client::parse_cp_target(&to)?;
    // 逐源解析并归侧:全部本机 or 全部同一台远端(混用即拒)
    let mut src_dev: Option<String> = None;
    let mut remote_paths: Vec<String> = Vec::new();
    let mut local_files: Vec<PathBuf> = Vec::new();
    for s in &sources {
        let (d, p) = client::parse_cp_target(s)?;
        match d {
            None => local_files.push(PathBuf::from(&p)),
            Some(dv) => {
                match &src_dev {
                    None => src_dev = Some(dv.clone()),
                    Some(prev) if *prev == dv => {}
                    Some(prev) => {
                        return Err(CliError {
                            code: 1,
                            message: format!("cp 的多个远端源须同一台设备:「{prev}」与「{dv}」不符"),
                        });
                    }
                }
                remote_paths.push(p);
            }
        }
    }
    if src_dev.is_some() && !local_files.is_empty() {
        return Err(CliError {
            code: 1,
            message: "cp 的源不能本机路径与远端目标混用".into(),
        });
    }
    match (src_dev.as_deref(), to_dev.as_deref()) {
        // ① download 糖:--to 是本机目录(无设备段)→ 远端源 → 本机落盘
        (Some(from), None) => {
            let dev_id = lookup_device(c, from).await?;
            let local_dir = PathBuf::from(&to_dir);
            let conflicts =
                client::scan_get_conflicts(c, &dev_id, &remote_paths, &local_dir).await?;
            let (skip, overwrite) = resolve_conflicts(&conflicts, conflict)?;
            let id = client::start_get(c, &dev_id, &remote_paths, &local_dir, &skip, &overwrite)
                .await?;
            finish_transfer(client::wait_job(c, &id).await?)?;
        }
        // ② upload 糖:源全本机 → --to 必须带设备段
        (None, Some(target)) => {
            let dev_id = lookup_device(c, target).await?;
            let conflicts = client::scan_put_conflicts(c, &dev_id, &to_dir, &local_files).await?;
            let (skip, overwrite) = resolve_conflicts(&conflicts, conflict)?;
            let id = client::start_put(c, &local_files, &dev_id, &to_dir, &skip, &overwrite)
                .await?;
            finish_transfer(client::wait_job(c, &id).await?)?;
        }
        // 源与目标都在本机:纯本机复制不经设备,cp 不管(指引用户)
        (None, None) => {
            return Err(CliError {
                code: 1,
                message: "cp 的源与目标都在本机:请直接用系统复制(cp 只管经设备的复制)".into(),
            });
        }
        // ③ 同设备糖:源设备 == 目标设备 → copy RPC(不经传输面)
        (Some(dev), Some(target)) if dev == target => {
            let dev_id = lookup_device(c, dev).await?;
            let conflicts =
                client::scan_cross_conflicts(c, &dev_id, &remote_paths, &dev_id, &to_dir).await?;
            if !conflicts.is_empty() {
                println!(
                    "目标处已存在 {} 个同名条目;同机复制冲突由接收端自动改名(--conflict 不适用)",
                    conflicts.len()
                );
            }
            let r = client::copy_remote_cmd(c, &dev_id, &remote_paths, &to_dir).await?;
            let failed = r["failed"].as_array().cloned().unwrap_or_default();
            if !failed.is_empty() {
                let detail: Vec<String> = failed
                    .iter()
                    .map(|f| {
                        format!(
                            "{}({}: {})",
                            f["path"].as_str().unwrap_or("?"),
                            f["code"].as_str().unwrap_or("?"),
                            f["message"].as_str().unwrap_or("")
                        )
                    })
                    .collect();
                return Err(CliError {
                    code: 1,
                    message: format!("同机复制部分失败: {}", detail.join("; ")),
                });
            }
            println!("同机复制完成:{} 项", r["copied"].as_u64().unwrap_or(0));
        }
        // ④ 真 remote_copy:源、目标两台不同设备(数据不经本机落地)
        (Some(from), Some(target)) => {
            let from_id = lookup_device(c, from).await?;
            let to_id = lookup_device(c, target).await?;
            let conflicts =
                client::scan_cross_conflicts(c, &from_id, &remote_paths, &to_id, &to_dir).await?;
            let (skip, overwrite) = resolve_conflicts(&conflicts, conflict)?;
            let id = client::start_remote_copy(
                c, &from_id, &remote_paths, &to_id, &to_dir, &skip, &overwrite,
            )
            .await?;
            finish_transfer(client::wait_job(c, &id).await?)?;
        }
    }
    Ok(())
}

/// 冲突策略落定(交互段留本文件):skip/overwrite 直接出策略表;ask 且有
/// 冲突 → 打印问询 + stdin 读答(库层出纯数据,非法应答重问)。取消返回
/// code 0(干净退出,不发起传输)。
fn resolve_conflicts(
    conflicts: &[String],
    mode: ConflictMode,
) -> Result<(Vec<String>, Vec<String>), CliError> {
    match mode {
        ConflictMode::Skip => Ok((conflicts.to_vec(), Vec::new())),
        ConflictMode::Overwrite => Ok((Vec::new(), conflicts.to_vec())),
        ConflictMode::Ask => {
            if conflicts.is_empty() {
                return Ok((Vec::new(), Vec::new()));
            }
            let prompt = client::conflict_prompt_data(conflicts);
            loop {
                println!("{}", prompt.text);
                print!("> ");
                let _ = std::io::stdout().flush();
                let mut line = String::new();
                std::io::stdin()
                    .read_line(&mut line)
                    .map_err(|e| CliError { code: 1, message: format!("读入应答失败: {e}") })?;
                match client::parse_conflict_answer(&line) {
                    Some(client::ConflictAction::SkipAll) => {
                        return Ok((conflicts.to_vec(), Vec::new()));
                    }
                    Some(client::ConflictAction::OverwriteAll) => {
                        return Ok((Vec::new(), conflicts.to_vec()));
                    }
                    Some(client::ConflictAction::Cancel) => {
                        return Err(CliError { code: 0, message: "已取消".into() });
                    }
                    None => continue, // 非法应答重问
                }
            }
        }
    }
}

/// 终态 → 退出码(全局约束钉死:done=0,其余 1 且 message 携带 error)。
fn finish_transfer(fin: serde_json::Value) -> Result<(), CliError> {
    let state = fin["state"].as_str().unwrap_or_default().to_string();
    if state == "done" {
        println!(
            "传输完成({} 个文件,共 {} 字节)",
            fin["files_done"].as_u64().unwrap_or(0),
            fin["bytes_total"].as_u64().unwrap_or(0),
        );
        Ok(())
    } else {
        let mut msg = format!("传输未完成(state={state})");
        if let Some(e) = fin["error"].as_str() {
            if !e.is_empty() {
                msg.push_str(&format!(":{e}"));
            }
        }
        Err(CliError { code: 1, message: msg })
    }
}
