use anyhow::Result;
use rusqlite::Connection;

pub fn open(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS accounts(
            id TEXT PRIMARY KEY, email TEXT UNIQUE NOT NULL, created_at INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS codes(
            email TEXT NOT NULL, code_hash TEXT NOT NULL, expires_at INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS devices(
            account_id TEXT NOT NULL, device_id TEXT PRIMARY KEY,
            pubkey TEXT NOT NULL, name TEXT NOT NULL, platform TEXT NOT NULL,
            created_at INTEGER NOT NULL);
         PRAGMA journal_mode=WAL;",
    )?;
    Ok(conn)
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
