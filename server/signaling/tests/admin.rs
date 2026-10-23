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

#[test]
fn enable_flips_and_is_idempotent() {
    let conn = db::open(":memory:").unwrap();
    assert!(admin::user_add(&conn, "a@b.c").unwrap());
    admin::relay_set_enabled(&conn, "a@b.c", true).unwrap();

    let enabled: i64 = conn
        .query_row(
            "SELECT enabled FROM relay_config WHERE account_id=(SELECT id FROM accounts WHERE email='a@b.c')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(enabled, 1, "enable 后 relay_config.enabled=1");

    // 幂等:再 enable 仍为 1,且不产生额外行
    admin::relay_set_enabled(&conn, "a@b.c", true).unwrap();
    let (enabled, rows): (i64, i64) = conn
        .query_row(
            "SELECT (SELECT enabled FROM relay_config WHERE account_id=(SELECT id FROM accounts WHERE email='a@b.c')), (SELECT COUNT(*) FROM relay_config)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(enabled, 1, "重复 enable 不得翻转或改动配置");
    assert_eq!(rows, 1);

    // 邮箱不存在 → Err(稳定文案 "no such account")
    let err = admin::relay_set_enabled(&conn, "ghost@x.y", true).unwrap_err();
    assert!(
        err.to_string().contains("no such account"),
        "报错需含 no such account:{err}"
    );
}

#[test]
fn limit_and_quota_persist() {
    let conn = db::open(":memory:").unwrap();
    assert!(admin::user_add(&conn, "a@b.c").unwrap());
    admin::relay_set_limit(&conn, "a@b.c", 512).unwrap();
    admin::relay_set_quota(&conn, "a@b.c", 5).unwrap();

    let (rate, quota): (i64, i64) = conn
        .query_row(
            "SELECT rate_kbps, quota_bytes FROM relay_config WHERE account_id=(SELECT id FROM accounts WHERE email='a@b.c')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(rate, 512, "set_limit 512 → 行值 512");
    assert_eq!(quota, 5 * 1024 * 1024 * 1024, "set_quota 5 → 5 GiB 字节数");

    // 0=不限:原样存 0
    admin::relay_set_limit(&conn, "a@b.c", 0).unwrap();
    admin::relay_set_quota(&conn, "a@b.c", 0).unwrap();
    let (rate, quota): (i64, i64) = conn
        .query_row(
            "SELECT rate_kbps, quota_bytes FROM relay_config WHERE account_id=(SELECT id FROM accounts WHERE email='a@b.c')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((rate, quota), (0, 0), "0=不限,原样存 0");
}

#[test]
fn legacy_account_shows_defaults() {
    let conn = db::open(":memory:").unwrap();
    assert!(admin::user_add(&conn, "a@b.c").unwrap());
    // 模拟生产遗留:user_add 未预建 relay_config 的老账号(行缺失)
    conn.execute("DELETE FROM relay_config", []).unwrap();

    let rows = admin::user_list(&conn).unwrap();
    assert_eq!(rows.len(), 1, "缺 relay_config 行的账号仍须列出");
    let row = &rows[0];
    assert_eq!(row.email, "a@b.c");
    assert!(!row.relay_enabled, "缺行账号按 disabled 呈现");
    assert_eq!(row.rate_kbps, admin::default_rate_kbps(), "缺行按默认限速呈现");
    assert_eq!(row.quota_bytes, admin::default_quota_bytes(), "缺行按默认配额呈现");

    // enable 触发补建:行回到 default_rate/default_quota + enabled=1,且只有一行
    admin::relay_set_enabled(&conn, "a@b.c", true).unwrap();
    let (enabled, rate, quota): (i64, i64, i64) = conn
        .query_row(
            "SELECT enabled, rate_kbps, quota_bytes FROM relay_config",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (enabled, rate, quota),
        (1, 1024, 10_737_418_240),
        "补建行:enable=1 + 其余取默认"
    );
}

#[test]
fn list_shows_current_period_usage() {
    let conn = db::open(":memory:").unwrap();
    assert!(admin::user_add(&conn, "a@b.c").unwrap());

    // 无 usage 行 → used_bytes 0
    assert_eq!(admin::user_list(&conn).unwrap()[0].used_bytes, 0);

    // 当期 usage 计入
    let id: String = conn
        .query_row("SELECT id FROM accounts WHERE email='a@b.c'", [], |r| r.get(0))
        .unwrap();
    let period = admin::current_period();
    conn.execute(
        "INSERT INTO relay_usage(account_id, period, bytes_forwarded) VALUES(?1, ?2, 123)",
        [&id, &period],
    )
    .unwrap();
    assert_eq!(admin::user_list(&conn).unwrap()[0].used_bytes, 123, "当期用量须反映");

    // 非当期行不计入
    conn.execute(
        "INSERT INTO relay_usage(account_id, period, bytes_forwarded) VALUES(?1, '2000-01', 999)",
        [&id],
    )
    .unwrap();
    assert_eq!(
        admin::user_list(&conn).unwrap()[0].used_bytes,
        123,
        "非当期 period 不得计入 used_bytes"
    );

    // period 取值域:UTC "YYYY-MM"
    assert_eq!(period.len(), 7, "period 须为 YYYY-MM 形状:{period}");
    assert_eq!(period.as_bytes()[4], b'-', "period 第 5 字符须为 '-':{period}");
}

#[test]
fn entitlement_disabled_by_default() {
    let conn = db::open(":memory:").unwrap();
    assert!(admin::user_add(&conn, "a@b.c").unwrap());
    let id: String = conn
        .query_row("SELECT id FROM accounts WHERE email='a@b.c'", [], |r| r.get(0))
        .unwrap();

    // 预置后未开启:(false, 默认限速, 默认配额全额余量)
    let (enabled, rate, remaining) = admin::relay_entitlement(&conn, &id).unwrap();
    assert_eq!(
        (enabled, rate, remaining),
        (false, 1024, 10_737_418_240),
        "未启用账号资格须为 disabled + 默认限速 + 默认余量"
    );
}

#[test]
fn entitlement_enabled_and_unlimited() {
    let conn = db::open(":memory:").unwrap();
    assert!(admin::user_add(&conn, "a@b.c").unwrap());
    admin::relay_set_enabled(&conn, "a@b.c", true).unwrap();
    admin::relay_set_quota(&conn, "a@b.c", 0).unwrap(); // 0=不限
    let id: String = conn
        .query_row("SELECT id FROM accounts WHERE email='a@b.c'", [], |r| r.get(0))
        .unwrap();

    let (enabled, rate, remaining) = admin::relay_entitlement(&conn, &id).unwrap();
    assert_eq!(
        (enabled, rate, remaining),
        (true, 1024, u64::MAX),
        "启用 + quota 0 须为 enabled + 默认限速 + u64::MAX(不限)"
    );
}

#[test]
fn entitlement_reflects_usage() {
    let conn = db::open(":memory:").unwrap();
    assert!(admin::user_add(&conn, "a@b.c").unwrap());
    let id: String = conn
        .query_row("SELECT id FROM accounts WHERE email='a@b.c'", [], |r| r.get(0))
        .unwrap();
    let period = admin::current_period();
    let mib: i64 = 1024 * 1024;
    conn.execute(
        "INSERT INTO relay_usage(account_id, period, bytes_forwarded) VALUES(?1, ?2, ?3)",
        rusqlite::params![id, period, mib],
    )
    .unwrap();

    let (enabled, rate, remaining) = admin::relay_entitlement(&conn, &id).unwrap();
    assert!(!enabled, "未 enable 时 usage 不改变 enabled");
    assert_eq!(rate, 1024);
    assert_eq!(
        remaining,
        10_737_418_240u64 - mib as u64,
        "余量 = 默认配额 - 当期用量"
    );

    // 非当期 period 不计入
    conn.execute(
        "INSERT INTO relay_usage(account_id, period, bytes_forwarded) VALUES(?1, '2000-01', 999)",
        [&id],
    )
    .unwrap();
    let (_, _, remaining) = admin::relay_entitlement(&conn, &id).unwrap();
    assert_eq!(remaining, 10_737_418_240u64 - mib as u64, "非当期用量不得计入");
}

#[test]
fn entitlement_missing_config_row_falls_back_to_defaults() {
    let conn = db::open(":memory:").unwrap();
    assert!(admin::user_add(&conn, "a@b.c").unwrap());
    // 模拟遗留账号:relay_config 行缺失
    conn.execute("DELETE FROM relay_config", []).unwrap();
    let id: String = conn
        .query_row("SELECT id FROM accounts WHERE email='a@b.c'", [], |r| r.get(0))
        .unwrap();

    let (enabled, rate, remaining) = admin::relay_entitlement(&conn, &id).unwrap();
    assert_eq!(
        (enabled, rate, remaining),
        (
            false,
            admin::default_rate_kbps(),
            admin::default_quota_bytes() as u64
        ),
        "缺 relay_config 行按 disabled + 默认限速 + 默认余量"
    );
}
