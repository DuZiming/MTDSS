//! 运维账号生命周期:预置/删除账号及其中继配置。
//! v2 起账号一律由运营方显式预置(不再"任意邮箱自动注册"),本模块提供
//! 管理 CLI 与认证收口共用的底层函数,不感知 AppState/Mutex。

use anyhow::{Context as _, Result};
use rusqlite::{Connection, OptionalExtension};

use crate::db::now;

/// 默认中继限速(kbps)
pub fn default_rate_kbps() -> u32 {
    1024
}

/// 默认月度流量配额(字节):10 GiB
pub fn default_quota_bytes() -> i64 {
    10_737_418_240
}

/// user_list 行:账号 + 其中继配置与当期用量。无 relay_config 行的遗留账号
/// 按 disabled + 默认值呈现(见 user_list)。
pub struct UserRow {
    pub email: String,
    pub account_id: String,
    pub relay_enabled: bool,
    pub rate_kbps: u32,
    pub quota_bytes: i64,
    pub used_bytes: i64, // 当期 relay_usage(LEFT JOIN COALESCE 0)
}

/// 当期计量周期:UTC "YYYY-MM"(relay_usage.period 的取值域)。
pub fn current_period() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!("{:04}-{:02}", now.year(), u8::from(now.month()))
}

/// 按 email 取账号 id;不存在时统一报 "no such account"(admin CLI 依赖的稳定文案)。
fn account_id_of(conn: &Connection, email: &str) -> Result<String> {
    conn.query_row("SELECT id FROM accounts WHERE email=?1", [email], |r| r.get(0))
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("no such account"))
}

/// 遗留账号兜底:relay_config 行缺失时按默认值补建(enabled=0),
/// 行已在则 no-op(INSERT OR IGNORE,account_id 为主键)。补建后 UPDATE 必然命中。
/// 单条语句,无需事务(user_add 的非原子预建留下的缺行账号由此收口)。
fn ensure_relay_config_row(conn: &Connection, account_id: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO relay_config(account_id, enabled, rate_kbps, quota_bytes, updated_at)
         VALUES(?1, 0, ?2, ?3, ?4)",
        rusqlite::params![account_id, default_rate_kbps(), default_quota_bytes(), now()],
    )?;
    Ok(())
}

/// 开/关中继。邮箱不存在 → Err("no such account");遗留账号缺 relay_config
/// 行时先补默认再改(UPDATE 生效)。
pub fn relay_set_enabled(conn: &Connection, email: &str, enabled: bool) -> Result<()> {
    let id = account_id_of(conn, email)?;
    ensure_relay_config_row(conn, &id)?;
    conn.execute(
        "UPDATE relay_config SET enabled=?2, updated_at=?3 WHERE account_id=?1",
        rusqlite::params![id, i64::from(enabled), now()],
    )?;
    Ok(())
}

/// 设中继限速(kbps)。0=不限,原样存 0。
pub fn relay_set_limit(conn: &Connection, email: &str, kbps: u32) -> Result<()> {
    let id = account_id_of(conn, email)?;
    ensure_relay_config_row(conn, &id)?;
    conn.execute(
        "UPDATE relay_config SET rate_kbps=?2, updated_at=?3 WHERE account_id=?1",
        rusqlite::params![id, kbps, now()],
    )?;
    Ok(())
}

/// 设月度流量配额(入参 GiB,存字节数 = gib * 1024^3)。0=不限,原样存 0。
pub fn relay_set_quota(conn: &Connection, email: &str, gib: u64) -> Result<()> {
    let id = account_id_of(conn, email)?;
    ensure_relay_config_row(conn, &id)?;
    // 字节换算防回绕:入参来自 CLI,离谱值显式报错而非静默截断
    let bytes = i64::try_from(gib.checked_mul(1 << 30).context("配额 GiB 过大")?)
        .context("配额字节数超出范围")?;
    conn.execute(
        "UPDATE relay_config SET quota_bytes=?2, updated_at=?3 WHERE account_id=?1",
        rusqlite::params![id, bytes, now()],
    )?;
    Ok(())
}

/// 全量账号清单,按 email 排序。无 relay_config 行的账号按 disabled+默认呈现;
/// used_bytes 取当期 relay_usage(LEFT JOIN,无行 COALESCE 0)。
pub fn user_list(conn: &Connection) -> Result<Vec<UserRow>> {
    // 默认值经 COALESCE 兜底写进 SQL,保持与 default_* 单一来源
    let sql = format!(
        "SELECT a.email, a.id,
                COALESCE(c.enabled, 0) != 0,
                COALESCE(c.rate_kbps, {rate}),
                COALESCE(c.quota_bytes, {quota}),
                COALESCE(u.bytes_forwarded, 0)
         FROM accounts a
         LEFT JOIN relay_config c ON c.account_id = a.id
         LEFT JOIN relay_usage u ON u.account_id = a.id AND u.period = ?1
         ORDER BY a.email",
        rate = default_rate_kbps(),
        quota = default_quota_bytes()
    );
    let period = current_period();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([&period], |r| {
            Ok(UserRow {
                email: r.get(0)?,
                account_id: r.get(1)?,
                relay_enabled: r.get(2)?,
                rate_kbps: r.get(3)?,
                quota_bytes: r.get(4)?,
                used_bytes: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 预置账号:新建 accounts 行并同步预建 relay_config 行(enabled=0 + 默认值,
/// 中继能力须运营方显式开启)。幂等:邮箱已存在时返回 false 且不改动任何
/// 既有配置。
pub fn user_add(conn: &Connection, email: &str) -> Result<bool> {
    // id 生成沿 auth.rs 现状:16 字节随机数 hex(32 字符)
    let id = hex::encode(rand::random::<[u8; 16]>());
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO accounts(id, email, created_at) VALUES(?1,?2,?3)",
        rusqlite::params![id, email, now()],
    )?;
    if inserted == 0 {
        return Ok(false); // 邮箱已存在:原样保留,不覆盖任何既有配置
    }
    conn.execute(
        "INSERT OR IGNORE INTO relay_config(account_id, enabled, rate_kbps, quota_bytes, updated_at)
         VALUES(?1, 0, ?2, ?3, ?4)",
        rusqlite::params![id, default_rate_kbps(), default_quota_bytes(), now()],
    )?;
    Ok(true)
}

/// 删除账号并级联清理其全部数据:devices/codes/relay_config/relay_usage/accounts,
/// 单事务保证原子性(未启用外键,级联靠显式 DELETE 表达)。codes 按 email 键,
/// 其余四表按 account_id。返回 true=已删除;false=账号本就不存在。
pub fn user_remove(conn: &Connection, email: &str) -> Result<bool> {
    // 签名收 &Connection(调用方经 state.db 的 MutexGuard 持锁),故用
    // unchecked_transaction 在共享引用上开事务;单连接单事务场景安全。
    let tx = conn.unchecked_transaction()?;
    let account_id: Option<String> = tx
        .query_row("SELECT id FROM accounts WHERE email=?1", [email], |r| r.get(0))
        .optional()?;
    let Some(account_id) = account_id else {
        return Ok(false); // 本就不存在
    };
    tx.execute("DELETE FROM devices WHERE account_id=?1", [&account_id])?;
    tx.execute("DELETE FROM codes WHERE email=?1", [email])?;
    tx.execute("DELETE FROM relay_config WHERE account_id=?1", [&account_id])?;
    tx.execute("DELETE FROM relay_usage WHERE account_id=?1", [&account_id])?;
    tx.execute("DELETE FROM accounts WHERE id=?1", [&account_id])?;
    tx.commit()?;
    Ok(true)
}
