//! admin 账号生命周期:直连 `:memory:` 库验证,不起服务器、不走 HTTP。

use signaling::admin;
use signaling::db;

#[test]
fn add_creates_account_and_default_relay_config() {
    let conn = db::open(":memory:").unwrap();
    assert!(admin::user_add(&conn, "a@b.c").unwrap());

    let (id, created_at): (String, i64) = conn
        .query_row(
            "SELECT id, created_at FROM accounts WHERE email='a@b.c'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(id.len(), 32, "账号 id 沿 auth.rs 的 16 字节 hex 惯例");
    assert!(created_at > 0);

    let (enabled, rate, quota): (i64, i64, i64) = conn
        .query_row(
            "SELECT enabled, rate_kbps, quota_bytes FROM relay_config WHERE account_id=?1",
            [&id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(enabled, 0, "预置账号默认关闭中继");
    assert_eq!(rate, 1024);
    assert_eq!(quota, 10_737_418_240);
}

#[test]
fn add_is_idempotent() {
    let conn = db::open(":memory:").unwrap();
    assert!(admin::user_add(&conn, "a@b.c").unwrap());
    // 第二次返回 false;relay_set 由 Task 2 引入,此处断 enabled 仍为 0、行数仍 1
    assert!(!admin::user_add(&conn, "a@b.c").unwrap());

    let accounts: i64 = conn
        .query_row("SELECT COUNT(*) FROM accounts WHERE email='a@b.c'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(accounts, 1, "幂等 add 不得重复建账号");

    // 全表只有 1 行 relay_config(不得给既有账号或幽灵 id 重复预建)
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM relay_config", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 1);
    let (enabled, rate, quota): (i64, i64, i64) = conn
        .query_row(
            "SELECT enabled, rate_kbps, quota_bytes FROM relay_config",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(enabled, 0, "幂等 add 不得改动既有中继配置");
    assert_eq!(rate, 1024);
    assert_eq!(quota, 10_737_418_240);
}

#[test]
fn remove_cascades_devices_and_codes() {
    let conn = db::open(":memory:").unwrap();
    assert!(admin::user_add(&conn, "a@b.c").unwrap());
    let id: String = conn
        .query_row("SELECT id FROM accounts WHERE email='a@b.c'", [], |r| r.get(0))
        .unwrap();
    // 手工铺满该账号的全部关联行:devices / codes / relay_usage
    conn.execute(
        "INSERT INTO devices(account_id, device_id, pubkey, name, platform, created_at)
         VALUES(?1, 'dev1', 'pk', 'n', 'test', 0)",
        [&id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO codes(email, code_hash, expires_at) VALUES('a@b.c', 'h', 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO relay_usage(account_id, period, bytes_forwarded) VALUES(?1, '2026-10', 5)",
        [&id],
    )
    .unwrap();

    assert!(admin::user_remove(&conn, "a@b.c").unwrap());
    // 五表对该账号全空
    for table in ["accounts", "devices", "codes", "relay_config", "relay_usage"] {
        let n: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "级联删除漏了表 {table}");
    }
    // 本就不存在
    assert!(!admin::user_remove(&conn, "a@b.c").unwrap());
}

#[test]
fn readd_mints_new_account_id() {
    let conn = db::open(":memory:").unwrap();
    assert!(admin::user_add(&conn, "a@b.c").unwrap());
    let first: String = conn
        .query_row("SELECT id FROM accounts WHERE email='a@b.c'", [], |r| r.get(0))
        .unwrap();
    assert!(admin::user_remove(&conn, "a@b.c").unwrap());
    assert!(admin::user_add(&conn, "a@b.c").unwrap());
    let second: String = conn
        .query_row("SELECT id FROM accounts WHERE email='a@b.c'", [], |r| r.get(0))
        .unwrap();
    assert_ne!(first, second, "重新预置必须拿到全新账号 id(白纸)");

    // 白纸:中继配置挂在且仅挂在新 id 上,且回到默认关闭
    let (enabled, owner): (i64, String) = conn
        .query_row(
            "SELECT enabled, account_id FROM relay_config",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(owner, second);
    assert_eq!(enabled, 0);
}
