use anyhow::Result;
use rusqlite::Connection;

pub fn open(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS accounts(
            id TEXT PRIMARY KEY, email TEXT UNIQUE NOT NULL, created_at INTEGER NOT NULL,
            device_limit INTEGER NOT NULL DEFAULT 3);
         CREATE TABLE IF NOT EXISTS codes(
            email TEXT NOT NULL, code_hash TEXT NOT NULL, expires_at INTEGER NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE IF NOT EXISTS devices(
            account_id TEXT NOT NULL, device_id TEXT PRIMARY KEY,
            pubkey TEXT NOT NULL, name TEXT NOT NULL, platform TEXT NOT NULL,
            created_at INTEGER NOT NULL, last_seen INTEGER NOT NULL DEFAULT 0);
         PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS relay_config(
           account_id TEXT PRIMARY KEY,
           enabled    INTEGER NOT NULL DEFAULT 0,
           rate_kbps  INTEGER NOT NULL DEFAULT 1024,
           quota_bytes INTEGER NOT NULL DEFAULT 10737418240,
           updated_at INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS relay_usage(
           account_id TEXT NOT NULL,
           period     TEXT NOT NULL,
           bytes_forwarded INTEGER NOT NULL DEFAULT 0,
           PRIMARY KEY(account_id, period));
         CREATE TABLE IF NOT EXISTS approvals(
            id TEXT PRIMARY KEY, account_id TEXT NOT NULL, pubkey TEXT NOT NULL,
            name TEXT NOT NULL, platform TEXT NOT NULL, code_hash TEXT NOT NULL,
            attempts INTEGER DEFAULT 0,
            state TEXT CHECK(state IN('pending','approved','denied','expired')),
            expires_at INTEGER);
         CREATE TABLE IF NOT EXISTS enroll_tokens(
            token TEXT PRIMARY KEY, account_id TEXT NOT NULL, pubkey TEXT NOT NULL,
            name TEXT NOT NULL, platform TEXT NOT NULL, expires_at INTEGER);
         CREATE TABLE IF NOT EXISTS qr_tokens(
            token TEXT PRIMARY KEY, account_id TEXT NOT NULL, claimed_by TEXT,
            candidate_pubkey TEXT NOT NULL, candidate_name TEXT NOT NULL,
            candidate_platform TEXT NOT NULL, state TEXT, expires_at INTEGER);
         CREATE TABLE IF NOT EXISTS account_kv(
            account_id TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
            updated_at INTEGER NOT NULL, PRIMARY KEY(account_id, key));",
    )?;
    migrate_legacy_columns(&conn)?;
    Ok(conn)
}

/// v3p1 迁移:存量老库补新列(幂等)。新库建表语句已含新列,探到即跳过;
/// 老库(CREATE TABLE IF NOT EXISTS 跳过既有表)由此补齐。SQLite 的
/// ALTER TABLE ADD COLUMN 无 IF NOT EXISTS,以 PRAGMA table_info 探列裁定。
/// 表名/列名均为本文件硬编码字面量,format! 无注入面。
fn migrate_legacy_columns(conn: &Connection) -> Result<()> {
    for (table, column, decl) in [
        ("accounts", "device_limit", "INTEGER NOT NULL DEFAULT 3"),
        ("devices", "last_seen", "INTEGER NOT NULL DEFAULT 0"),
        ("codes", "attempts", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        if !column_exists(conn, table, column)? {
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl};"))?;
        }
    }
    Ok(())
}

/// PRAGMA table_info 探列:name 是第 2 列(cid, name, type, notnull, dflt_value, pk)。
fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
