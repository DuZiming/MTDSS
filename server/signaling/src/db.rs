use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};

pub fn open(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS secrets(
            name TEXT PRIMARY KEY, value BLOB NOT NULL);
         CREATE TABLE IF NOT EXISTS accounts(
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

/// JWT 签名密钥:首启生成入库、此后读库复用(2026-11-05 修复——此前每次
/// 进程启动 rand::random() 重掷,服务端重启=全端令牌作废+客户端两拒收口
/// 清会话集体登出;实机场三次连续重启炸出该 V2 潜伏雷)。
pub fn load_or_create_jwt_secret(conn: &Connection) -> Result<[u8; 32]> {
    if let Some(blob) = conn
        .query_row("SELECT value FROM secrets WHERE name='jwt'", [], |r| {
            r.get::<_, Vec<u8>>(0)
        })
        .optional()?
    {
        let arr: [u8; 32] = blob
            .try_into()
            .map_err(|e| anyhow::anyhow!("jwt secret 库内长度异常: {e:?}"))?;
            // ↑ try_into 错误类型=原 Vec(无 Display),用 Debug 呈现
        return Ok(arr);
    }
    let secret: [u8; 32] = rand::random();
    conn.execute(
        "INSERT INTO secrets(name, value) VALUES('jwt', ?1)",
        [&secret],
    )?;
    Ok(secret)
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 重启稳定性:同一库文件两次 open→load 应得同一密钥;不同库各自独立。
    /// (2026-11-05 修复的回归锚:此前每次进程启动重掷密钥,重启=全端登出。)
    #[test]
    fn jwt_secret_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.db");
        let p = path.to_str().unwrap();
        let first = load_or_create_jwt_secret(&open(p).unwrap()).unwrap();
        let second = load_or_create_jwt_secret(&open(p).unwrap()).unwrap();
        assert_eq!(first, second, "同库重启必须复用同一 JWT 密钥");
        let other = load_or_create_jwt_secret(&open(
            dir.path().join("other.db").to_str().unwrap(),
        )
        .unwrap())
        .unwrap();
        assert_ne!(first, other, "不同库应各自独立生成密钥");
    }
}
