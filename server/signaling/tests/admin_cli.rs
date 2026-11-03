//! mtdss-admin CLI 端到端:真子进程跑二进制(`CARGO_BIN_EXE_mtdss-admin`),
//! 临时目录内建库,按语法钉死的命令行逐条串联验证退出码与人读输出。

use std::process::Command;

/// 跑一条 `mtdss-admin --db <db> <args>`,返回 (退出码, stdout, stderr)。
fn run(db: &std::path::Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_mtdss-admin"))
        .arg("--db")
        .arg(db)
        .args(args)
        .output()
        .expect("spawn mtdss-admin");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn cli_happy_chain() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("signaling.db");

    // user add → exit 0,stdout 含 "a@b.c"
    let (code, out, err) = run(&db, &["user", "add", "a@b.c"]);
    assert_eq!(code, 0, "user add 须成功;stderr: {err}");
    assert!(out.contains("a@b.c"), "add 确认行须含邮箱: {out}");

    // user list → 含 a@b.c 且含 "disabled"(新账号中继默认关闭)
    let (code, out, err) = run(&db, &["user", "list"]);
    assert_eq!(code, 0, "user list 须成功;stderr: {err}");
    let cols: Vec<&str> = out
        .lines()
        .find(|l| l.contains("a@b.c"))
        .expect("list 须含 a@b.c 行")
        .split_whitespace()
        .collect();
    assert_eq!(cols[2], "disabled", "新账号中继列应为 disabled: {out}");

    // relay enable/limit/quota → 全部 exit 0
    for args in [
        vec!["relay", "enable", "a@b.c"],
        vec!["relay", "limit", "a@b.c", "512"],
        vec!["relay", "quota", "a@b.c", "5"],
    ] {
        let (code, _out, err) = run(&db, &args);
        assert_eq!(code, 0, "{args:?} 须成功;stderr: {err}");
    }

    // user list --with-usage → 此时限速列应为 "512"、配额列 "5"(不再是默认 1024/10)
    let (code, out, err) = run(&db, &["user", "list", "--with-usage"]);
    assert_eq!(code, 0, "list --with-usage 须成功;stderr: {err}");
    let cols: Vec<&str> = out
        .lines()
        .find(|l| l.contains("a@b.c"))
        .expect("list --with-usage 须含 a@b.c 行")
        .split_whitespace()
        .collect();
    assert_eq!(cols[2], "enabled", "enable 后中继列应为 enabled: {out}");
    assert_eq!(cols[3], "512", "限速列应为 512:{out}");
    assert_eq!(cols[4], "5", "配额列应为 5(GiB):{out}");

    // relay disable → 0;user list → 中继列回到 "disabled"(disable 语法钉死覆盖)
    let (code, _out, err) = run(&db, &["relay", "disable", "a@b.c"]);
    assert_eq!(code, 0, "relay disable 须成功;stderr: {err}");
    let (code, out, err) = run(&db, &["user", "list"]);
    assert_eq!(code, 0, "disable 后 user list 须成功;stderr: {err}");
    let cols: Vec<&str> = out
        .lines()
        .find(|l| l.contains("a@b.c"))
        .expect("disable 后 list 仍须含 a@b.c 行")
        .split_whitespace()
        .collect();
    assert_eq!(cols[2], "disabled", "disable 后中继列应回到 disabled: {out}");

    // user remove → 0;user list → 不含 a@b.c
    let (code, _out, err) = run(&db, &["user", "remove", "a@b.c"]);
    assert_eq!(code, 0, "user remove 须成功;stderr: {err}");
    let (code, out, err) = run(&db, &["user", "list"]);
    assert_eq!(code, 0, "删除后 user list 须成功;stderr: {err}");
    assert!(!out.contains("a@b.c"), "删除后清单不得再含 a@b.c: {out}");
}

#[test]
fn cli_rejects_unknown_command() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("signaling.db");

    // 未知命令 → 非 0 退出,且 USAGE 落到 stderr
    let (code, _out, err) = run(&db, &["frobnicate"]);
    assert_ne!(code, 0, "未知命令须非 0 退出");
    assert!(err.contains("usage"), "stderr 须含 usage: {err}");

    // 已知动词配错参数形状同样拒绝(USAGE 到 stderr,非 0)
    let (code, _out, err) = run(&db, &["user", "add"]);
    assert_ne!(code, 0, "缺邮箱参数须非 0 退出");
    assert!(err.contains("usage"), "stderr 须含 usage: {err}");
}

// ---- v3p1 追加:库表迁移 + admin 终端管理三命令(Task 1)----

/// PRAGMA table_info 探列(与 db.rs 迁移同款手法):返回表的全部列名。
fn columns_of(conn: &rusqlite::Connection, table: &str) -> Vec<String> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})")).unwrap();
    let names = stmt.query_map([], |r| r.get::<_, String>(1)).unwrap();
    names.map(|n| n.unwrap()).collect()
}

#[test]
fn cli_device_and_limit_chain() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("signaling.db");

    // user add → exit 0
    let (code, _out, err) = run(&db, &["user", "add", "a@b.c"]);
    assert_eq!(code, 0, "user add 须成功;stderr: {err}");

    // user list 增列:新账号 device_limit 默认 3(第 6 列)
    let (code, out, err) = run(&db, &["user", "list"]);
    assert_eq!(code, 0, "user list 须成功;stderr: {err}");
    let cols: Vec<&str> = out
        .lines()
        .find(|l| l.contains("a@b.c"))
        .expect("list 须含 a@b.c 行")
        .split_whitespace()
        .collect();
    assert_eq!(cols[5], "3", "新账号 device_limit 列应为默认 3:{out}");

    // set-device-limit 边界:改 5 → 0(不限)→ 改回 3,每步回读
    for n in ["5", "0", "3"] {
        let (code, _out, err) = run(&db, &["user", "set-device-limit", "a@b.c", n]);
        assert_eq!(code, 0, "set-device-limit {n} 须成功;stderr: {err}");
        let (code, out, err) = run(&db, &["user", "list"]);
        assert_eq!(code, 0, "user list 须成功;stderr: {err}");
        let cols: Vec<&str> = out
            .lines()
            .find(|l| l.contains("a@b.c"))
            .expect("list 须含 a@b.c 行")
            .split_whitespace()
            .collect();
        assert_eq!(cols[5], n, "device_limit 列应为 {n}:{out}");
    }

    // 直接铺两台设备(CLI 无插设备命令):一台 last_seen=now(在线)、一台 last_seen=0(离线)
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        let id: String = conn
            .query_row("SELECT id FROM accounts WHERE email='a@b.c'", [], |r| r.get(0))
            .unwrap();
        conn.execute(
            "INSERT INTO devices(account_id, device_id, pubkey, name, platform, created_at, last_seen)
             VALUES(?1,'dev-live','pk','Laptop','windows',1000,?2)",
            rusqlite::params![id, signaling::db::now()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO devices(account_id, device_id, pubkey, name, platform, created_at, last_seen)
             VALUES(?1,'dev-stale','pk','Phone','android',2000,0)",
            [&id],
        )
        .unwrap();
    }

    // device list → 在线列按 last_seen≤70s 近似判定,表头注明近似(approx)
    let (code, out, err) = run(&db, &["device", "list", "a@b.c"]);
    assert_eq!(code, 0, "device list 须成功;stderr: {err}");
    assert!(out.contains("approx"), "在线列表头须注明近似:{out}");
    let live = out
        .lines()
        .find(|l| l.contains("dev-live"))
        .expect("list 须含 dev-live:{out}");
    assert_eq!(
        live.split_whitespace().last(),
        Some("online"),
        "last_seen≈now 的设备应判在线:{out}"
    );
    let stale = out
        .lines()
        .find(|l| l.contains("dev-stale"))
        .expect("list 须含 dev-stale:{out}");
    assert_eq!(
        stale.split_whitespace().last(),
        Some("offline"),
        "last_seen=0 的设备应判离线:{out}"
    );

    // device unbind 存在设备 → 0;list 只剩另一台
    let (code, _out, err) = run(&db, &["device", "unbind", "a@b.c", "dev-live"]);
    assert_eq!(code, 0, "unbind 须成功;stderr: {err}");
    let (code, out, err) = run(&db, &["device", "list", "a@b.c"]);
    assert_eq!(code, 0, "unbind 后 device list 须成功;stderr: {err}");
    assert!(!out.contains("dev-live"), "unbind 后不得再含 dev-live:{out}");
    assert!(out.contains("dev-stale"), "unbind 只删目标设备:{out}");

    // unbind 不存在设备 → 非 0(Err)
    let (code, _out, _err) = run(&db, &["device", "unbind", "a@b.c", "dev-ghost"]);
    assert_ne!(code, 0, "unbind 不存在设备须报错");
}

#[test]
fn migration_is_idempotent_and_lands_all_objects() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("signaling.db");

    // 连跑两次 init 均须无错(幂等)
    signaling::db::open(db.to_str().unwrap()).unwrap();
    signaling::db::open(db.to_str().unwrap()).unwrap();

    let conn = rusqlite::Connection::open(&db).unwrap();

    // 三新列在位
    assert!(columns_of(&conn, "accounts").contains(&"device_limit".to_string()));
    assert!(columns_of(&conn, "devices").contains(&"last_seen".to_string()));
    assert!(columns_of(&conn, "codes").contains(&"attempts".to_string()));

    // 四新表在位且为空
    for table in ["approvals", "enroll_tokens", "qr_tokens", "account_kv"] {
        let n: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "新表 {table} 须在位且初始为空");
    }

    // DDL 钉死:approvals.state 的 CHECK 在位(非法值拒、合法值过)
    let err = conn
        .execute(
            "INSERT INTO approvals(id,account_id,pubkey,name,platform,code_hash,attempts,state,expires_at)
             VALUES('x','a','p','n','pf','h',0,'bogus',1)",
            [],
        )
        .unwrap_err();
    assert!(err.to_string().contains("CHECK"), "非法 state 须被 CHECK 拒:{err}");
    conn.execute(
        "INSERT INTO approvals(id,account_id,pubkey,name,platform,code_hash,attempts,state,expires_at)
         VALUES('x','a','p','n','pf','h',0,'pending',1)",
        [],
    )
    .unwrap();
}

#[test]
fn migration_upgrades_legacy_db_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("signaling.db");

    // 手工按 v2 旧形建三表(无新列),铺存量行
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE accounts(id TEXT PRIMARY KEY, email TEXT UNIQUE NOT NULL, created_at INTEGER NOT NULL);
             CREATE TABLE codes(email TEXT NOT NULL, code_hash TEXT NOT NULL, expires_at INTEGER NOT NULL);
             CREATE TABLE devices(
                account_id TEXT NOT NULL, device_id TEXT PRIMARY KEY,
                pubkey TEXT NOT NULL, name TEXT NOT NULL, platform TEXT NOT NULL,
                created_at INTEGER NOT NULL);",
        )
        .unwrap();
        conn.execute("INSERT INTO accounts(id,email,created_at) VALUES('i1','old@x.y',100)", [])
            .unwrap();
        conn.execute(
            "INSERT INTO devices(account_id,device_id,pubkey,name,platform,created_at)
             VALUES('i1','d1','pk','n','win',100)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO codes(email,code_hash,expires_at) VALUES('old@x.y','h',200)", [])
            .unwrap();
    }

    // db::open 迁移:补列,存量行取默认值(limit=3/seen=0/attempts=0)
    signaling::db::open(db.to_str().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    let (limit, seen, attempts): (i64, i64, i64) = conn
        .query_row(
            "SELECT (SELECT device_limit FROM accounts WHERE id='i1'),
                    (SELECT last_seen FROM devices WHERE device_id='d1'),
                    (SELECT attempts FROM codes WHERE email='old@x.y')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (limit, seen, attempts),
        (3, 0, 0),
        "存量行迁移须取默认:limit=3/seen=0/attempts=0"
    );

    // 第二次 open 仍无错(幂等)
    signaling::db::open(db.to_str().unwrap()).unwrap();
}

#[test]
fn user_remove_cascades_new_tables() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("signaling.db");
    let (code, _out, err) = run(&db, &["user", "add", "a@b.c"]);
    assert_eq!(code, 0, "user add 须成功;stderr: {err}");

    // 铺满 devices + 四新表的关联行
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        let id: String = conn
            .query_row("SELECT id FROM accounts WHERE email='a@b.c'", [], |r| r.get(0))
            .unwrap();
        conn.execute(
            "INSERT INTO devices(account_id,device_id,pubkey,name,platform,created_at,last_seen)
             VALUES(?1,'d1','pk','n','win',1,0)",
            [&id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO approvals(id,account_id,pubkey,name,platform,code_hash,attempts,state,expires_at)
             VALUES('s1',?1,'p','n','win','h',0,'pending',99)",
            [&id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO enroll_tokens(token,account_id,pubkey,name,platform,expires_at)
             VALUES('t1',?1,'p','n','win',99)",
            [&id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO qr_tokens(token,account_id,claimed_by,candidate_pubkey,candidate_name,candidate_platform,state,expires_at)
             VALUES('q1',?1,NULL,'p','n','win','pending',99)",
            [&id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO account_kv(account_id,key,value,updated_at) VALUES(?1,'k','v',1)",
            [&id],
        )
        .unwrap();
    }

    let (code, _out, err) = run(&db, &["user", "remove", "a@b.c"]);
    assert_eq!(code, 0, "user remove 须成功;stderr: {err}");

    // 级联:devices + 四新表对该账号全空
    let conn = rusqlite::Connection::open(&db).unwrap();
    for table in ["devices", "approvals", "enroll_tokens", "qr_tokens", "account_kv"] {
        let n: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "级联删除漏了表 {table}");
    }
}
