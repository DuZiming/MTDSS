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
