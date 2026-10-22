//! mtdss-admin:运维账号/中继配置 CLI,语法钉死(见 USAGE):
//!
//! ```text
//! mtdss-admin [--db <path>] user add <email> | user remove <email> | user list [--with-usage]
//! mtdss-admin [--db <path>] relay enable <email> | relay disable <email> | relay limit <email> <kbps> | relay quota <email> <GB>
//! ```
//!
//! 纯参数解析薄壳:全部逻辑走 [`signaling::admin`]。手工解析沿 main.rs 风格
//! (USAGE 常量 + bail),不引 clap;输出面向人读(每命令一行确认;list 为
//! 对齐表格,`--with-usage` 追加当期用量列)。

use signaling::admin;

const USAGE: &str = "usage: mtdss-admin [--db <path>] user add <email> | user remove <email> | user list [--with-usage] \
                     | mtdss-admin [--db <path>] relay enable <email> | relay disable <email> \
                     | relay limit <email> <kbps> | relay quota <email> <GB>";

/// 已验形的子命令(数值已解析):main 先解析后开库,坏参数不得产生 DB 文件副作用。
enum Cmd {
    UserAdd(String),
    UserRemove(String),
    UserList { with_usage: bool },
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

/// 对齐表格:list 默认五列(账号/中继/限速 kbps/配额 GiB),`--with-usage`
/// 追加当期用量列(字节)。配额按 GiB 整数呈现(默认 10 GiB → "10")。
fn print_user_list(rows: &[admin::UserRow], with_usage: bool) {
    let w = rows.iter().map(|r| r.email.len()).max().unwrap_or(0).max(5);
    if with_usage {
        println!(
            "{:<w$} {:<32} {:<8} {:>10} {:>10} {:>10}",
            "email", "account", "relay", "limit_kbps", "quota_gib", "used_bytes",
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
                "{:<w$} {:<32} {:<8} {:>10} {:>10}",
                r.email,
                r.account_id,
                onoff(r.relay_enabled),
                r.rate_kbps,
                r.quota_bytes >> 30,
                w = w
            );
        }
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
