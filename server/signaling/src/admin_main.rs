//! mtdss-admin:运维账号/中继配置 CLI,语法钉死(见 USAGE):
//!
//! ```text
//! mtdss-admin [--db <path>] user add <email> | user remove <email> | user list [--with-usage]
//!             | user set-device-limit <email> <n>
//! mtdss-admin [--db <path>] device list <email> | device unbind <email> <device_id>
//! mtdss-admin [--db <path>] relay enable <email> | relay disable <email> | relay limit <email> <kbps> | relay quota <email> <GB>
//! ```
//!
//! 纯参数解析薄壳:全部逻辑走 [`signaling::admin`]。手工解析沿 main.rs 风格
//! (USAGE 常量 + bail),不引 clap;输出面向人读(每命令一行确认;list 为
//! 对齐表格,`--with-usage` 追加当期用量列)。

use signaling::admin;

const USAGE: &str = "usage: mtdss-admin [--db <path>] user add <email> | user remove <email> | user list [--with-usage] \
                     | user set-device-limit <email> <n> \
                     | mtdss-admin [--db <path>] device list <email> | device unbind <email> <device_id> \
                     | mtdss-admin [--db <path>] relay enable <email> | relay disable <email> \
                     | relay limit <email> <kbps> | relay quota <email> <GB>";

/// 已验形的子命令(数值已解析):main 先解析后开库,坏参数不得产生 DB 文件副作用。
enum Cmd {
    UserAdd(String),
    UserRemove(String),
    UserList { with_usage: bool },
    UserSetDeviceLimit { email: String, n: u32 },
    DeviceList { email: String },
    DeviceUnbind { email: String, device_id: String },
    RelayEnabled { email: String, enabled: bool },
    RelayLimit { email: String, kbps: u32 },
    RelayQuota { email: String, gib: u64 },
}

fn parse_num<T: std::str::FromStr>(s: &str, what: &str) -> anyhow::Result<T> {
    s.parse().map_err(|_| anyhow::anyhow!("bad {what} {s:?}; {USAGE}"))
}

fn parse_cmd(rest: &[String]) -> anyhow::Result<Cmd> {
    // 字面量切片模式需 &str 元素,先收拢
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
    Ok(match rest.as_slice() {
        ["user", "add", email] => Cmd::UserAdd(email.to_string()),
        ["user", "remove", email] => Cmd::UserRemove(email.to_string()),
        ["user", "list"] => Cmd::UserList { with_usage: false },
        ["user", "list", "--with-usage"] => Cmd::UserList { with_usage: true },
        ["user", "set-device-limit", email, n] => Cmd::UserSetDeviceLimit {
            email: email.to_string(),
            n: parse_num(n, "device limit")?,
        },
        ["device", "list", email] => Cmd::DeviceList {
            email: email.to_string(),
        },
        ["device", "unbind", email, device_id] => Cmd::DeviceUnbind {
            email: email.to_string(),
            device_id: device_id.to_string(),
        },
        ["relay", "enable", email] => Cmd::RelayEnabled {
            email: email.to_string(),
            enabled: true,
        },
        ["relay", "disable", email] => Cmd::RelayEnabled {
            email: email.to_string(),
            enabled: false,
        },
        ["relay", "limit", email, kbps] => Cmd::RelayLimit {
            email: email.to_string(),
            kbps: parse_num(kbps, "kbps")?,
        },
        ["relay", "quota", email, gib] => Cmd::RelayQuota {
            email: email.to_string(),
            gib: parse_num(gib, "GiB")?,
        },
        _ => anyhow::bail!("unknown command/args {rest:?}; {USAGE}"),
    })
}

fn onoff(enabled: bool) -> &'static str {
    if enabled { "enabled" } else { "disabled" }
}

/// 在线近似窗口(spec §3.7):last_seen 距今 ≤70s 视为在线。last_seen 由
/// presence 注册/注销与 keepalive tick 滚动续写,70s 给足余量。
const ONLINE_WINDOW_SECS: i64 = 70;

/// 对齐表格:list 默认六列(账号/中继/限速 kbps/配额 GiB/终端上限),`--with-usage`
/// 追加当期用量列(字节)。配额按 GiB 整数呈现(默认 10 GiB → "10")。
fn print_user_list(rows: &[admin::UserRow], with_usage: bool) {
    let w = rows.iter().map(|r| r.email.len()).max().unwrap_or(0).max(5);
    if with_usage {
        println!(
            "{:<w$} {:<32} {:<8} {:>10} {:>10} {:>10} {:>10}",
            "email", "account", "relay", "limit_kbps", "quota_gib", "dev_limit", "used_bytes",
            w = w
        );
        for r in rows {
            println!(
                "{:<w$} {:<32} {:<8} {:>10} {:>10} {:>10} {:>10}",
                r.email,
                r.account_id,
                onoff(r.relay_enabled),
                r.rate_kbps,
                r.quota_bytes >> 30,
                r.device_limit,
                r.used_bytes,
                w = w
            );
        }
    } else {
        println!(
            "{:<w$} {:<32} {:<8} {:>10} {:>10}",
            "email", "account", "relay", "limit_kbps", "quota_gib",
            w = w
        );
        for r in rows {
            println!(
                "{:<w$} {:<32} {:<8} {:>10} {:>10} {:>10}",
                r.email,
                r.account_id,
                onoff(r.relay_enabled),
                r.rate_kbps,
                r.quota_bytes >> 30,
                r.device_limit,
                w = w
            );
        }
    }
}

/// 对齐表格:device_id/名称/平台/创建时间(epoch 秒)/在线。在线列为近似判定
/// (last_seen≤70s),表头以 online(approx) 注明。
fn print_device_list(rows: &[admin::DeviceRow]) {
    let now = signaling::db::now();
    let w_id = rows.iter().map(|r| r.device_id.len()).max().unwrap_or(0).max(6);
    let w_name = rows.iter().map(|r| r.name.len()).max().unwrap_or(0).max(4);
    let w_platform = rows.iter().map(|r| r.platform.len()).max().unwrap_or(0).max(8);
    println!(
        "{:<w_id$} {:<w_name$} {:<w_platform$} {:>10} {:>14}",
        "device", "name", "platform", "created", "online(approx)",
        w_id = w_id,
        w_name = w_name,
        w_platform = w_platform
    );
    for r in rows {
        let online = now - r.last_seen <= ONLINE_WINDOW_SECS;
        println!(
            "{:<w_id$} {:<w_name$} {:<w_platform$} {:>10} {:>14}",
            r.device_id,
            r.name,
            r.platform,
            r.created_at,
            if online { "online" } else { "offline" },
            w_id = w_id,
            w_name = w_name,
            w_platform = w_platform
        );
    }
}

fn exec(conn: &rusqlite::Connection, cmd: Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::UserAdd(email) => {
            if admin::user_add(conn, &email)? {
                println!("user added: {email}");
            } else {
                println!("user exists, kept unchanged: {email}");
            }
        }
        Cmd::UserRemove(email) => {
            if !admin::user_remove(conn, &email)? {
                anyhow::bail!("no such account: {email}");
            }
            println!("user removed: {email}");
        }
        Cmd::UserList { with_usage } => print_user_list(&admin::user_list(conn)?, with_usage),
        Cmd::UserSetDeviceLimit { email, n } => {
            admin::user_set_device_limit(conn, &email, n)?;
            println!("device limit set: {email} {n}");
        }
        Cmd::DeviceList { email } => print_device_list(&admin::device_list(conn, &email)?),
        Cmd::DeviceUnbind { email, device_id } => {
            if !admin::device_unbind(conn, &email, &device_id)? {
                anyhow::bail!("no such device: {device_id}");
            }
            println!("device unbound: {email} {device_id}");
        }
        Cmd::RelayEnabled { email, enabled } => {
            admin::relay_set_enabled(conn, &email, enabled)?;
            println!("relay {}: {email}", onoff(enabled));
        }
        Cmd::RelayLimit { email, kbps } => {
            admin::relay_set_limit(conn, &email, kbps)?;
            println!("relay limit set: {email} {kbps} kbps");
        }
        Cmd::RelayQuota { email, gib } => {
            admin::relay_set_quota(conn, &email, gib)?;
            println!("relay quota set: {email} {gib} GiB");
        }
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    // `--db` 可前置(语法钉死形态);其余 token 全量留给子命令解析。
    let mut db_path = "signaling.db".to_string();
    let mut rest: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--db" => {
                db_path = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--db needs a value; {USAGE}"))?
            }
            other => rest.push(other.to_string()),
        }
    }

    let cmd = parse_cmd(&rest)?; // 先验形(含数值解析),后开库
    let conn = signaling::db::open(&db_path)?;
    exec(&conn, cmd)
}
