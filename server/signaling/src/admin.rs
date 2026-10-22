//! 运维账号生命周期:预置/删除账号及其中继配置。
//! v2 起账号一律由运营方显式预置(不再"任意邮箱自动注册"),本模块提供
//! 管理 CLI 与认证收口共用的底层函数,不感知 AppState/Mutex。

use anyhow::Result;
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
