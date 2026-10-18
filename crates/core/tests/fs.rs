//! 远程浏览 FS 服务(`core::fs`):根卷/列表/stat/保留名过滤/截断。
//!
//! 全部走真实临时目录(tempfile):混合文件/目录与 `.mtdss`/`.mtdss-part`
//! 保留名滤除、目录优先排序、mtime/size 字段、`path` 字段可再入、
//! 截断注入 cap、各平台根卷形态、错误码映射;Unix 侧另有符号链接
//! 不跟随与 chmod 000 → `PermissionDenied`(运行于 root 时自跳过)。

use mtdss_core::fs::{list_dir, list_dir_capped, stat_path, DirEntryData, FsError, LIST_ENTRY_CAP};
use std::path::Path;

// ---------- 测试工具 ----------

/// 混合条目临时目录:2 目录 + 2 文件 + `.mtdss` 目录 + `.mtdss-part`
/// 文件(后两者应被滤除)。返回 (目录, 期望目录名, 期望文件名)。
fn seed_mixed() -> (tempfile::TempDir, Vec<&'static str>, Vec<&'static str>) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("zdir")).unwrap();
    std::fs::create_dir(dir.path().join("adir")).unwrap();
    std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();
    std::fs::write(dir.path().join("b.bin"), b"abc").unwrap();
    std::fs::create_dir(dir.path().join(".mtdss")).unwrap();
    std::fs::write(dir.path().join(".mtdss-part"), b"junk").unwrap();
    (dir, vec!["adir", "zdir"], vec!["a.txt", "b.bin"])
}

fn join(base: &str, name: &str) -> String {
    Path::new(base).join(name).to_string_lossy().into_owned()
}

fn find<'a>(entries: &'a [DirEntryData], name: &str) -> &'a DirEntryData {
    entries.iter().find(|e| e.name == name).expect("entry present")
}

// ---------- 列表:过滤/排序/字段 ----------

#[test]
fn list_filters_reserved_and_sorts_dirs_first() {
    let (dir, dirs, files) = seed_mixed();
    let out = list_dir(dir.path().to_str().unwrap()).unwrap();
    assert!(!out.truncated);
    let names: Vec<&str> = out.entries.iter().map(|e| e.name.as_str()).collect();
    // 期望序:目录优先、组内字节升序;.mtdss/.mtdss-part 已滤除
    let mut expect = dirs;
    expect.extend(files);
    assert_eq!(names, expect);
}

#[test]
fn entry_fields_sizes_mtime_and_reenterable_paths() {
    let (dir, _, _) = seed_mixed();
    let out = list_dir(dir.path().to_str().unwrap()).unwrap();
    assert_eq!(out.entries.len(), 4);
    // path 字段可再入:逐条 stat 成功且同形态(列表/单查判定一致)
    for e in &out.entries {
        let st = stat_path(&e.path).unwrap();
        assert_eq!(st.name, e.name, "name consistency for {}", e.name);
        assert_eq!(st.path, e.path);
        assert_eq!(st.is_dir, e.is_dir);
        assert_eq!(st.size, e.size);
    }
    let a = find(&out.entries, "a.txt");
    assert_eq!(a.size, 5, "文件 size = 内容长度");
    assert!(a.mtime_ms > 0, "新文件 mtime 非零");
    let b = find(&out.entries, "b.bin");
    assert_eq!(b.size, 3);
    assert!(b.mtime_ms > 0);
    let adir = find(&out.entries, "adir");
    assert!(adir.is_dir);
    assert_eq!(adir.size, 0, "目录 size 恒 0");
    assert!(adir.mtime_ms > 0, "新建目录 mtime 非零");
}

#[test]
fn list_dir_capped_truncates() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..7 {
        std::fs::write(dir.path().join(format!("f{i}.txt")), b"x").unwrap();
    }
    let p = dir.path().to_str().unwrap();
    // 7 条 cap=5 → 恰 5 条 + truncated
    let capped = list_dir_capped(p, 5).unwrap();
    assert_eq!(capped.entries.len(), 5);
    assert!(capped.truncated);
    // 默认 cap 不截断
    let full = list_dir(p).unwrap();
    assert_eq!(full.entries.len(), 7);
    assert!(!full.truncated);
    // 恰好等于 cap 不算截断("超 cap"才截)
    let exact = list_dir_capped(p, 7).unwrap();
    assert_eq!(exact.entries.len(), 7);
    assert!(!exact.truncated);
}

#[test]
fn entry_cap_constant_pinned_for_ctrl_channel() {
    assert_eq!(LIST_ENTRY_CAP, 10_000);
}

// ---------- stat:文件/目录/盘根 ----------

#[test]
fn stat_file_dir_and_root() {
    let (dir, _, _) = seed_mixed();
    let base = dir.path().to_str().unwrap();

    let f = stat_path(&join(base, "a.txt")).unwrap();
    assert!(!f.is_dir);
    assert_eq!(f.size, 5);
    assert_eq!(f.name, "a.txt");
    assert!(f.mtime_ms > 0);

    let d = stat_path(&join(base, "zdir")).unwrap();
    assert!(d.is_dir);
    assert_eq!(d.size, 0);

    #[cfg(windows)]
    {
        // 盘根合法;显示名与根卷列表条目一致
        let r = stat_path("C:\\").unwrap();
        assert!(r.is_dir);
        assert_eq!(r.name, "C:");
        assert_eq!(r.path, "C:\\");
        // 裸盘符 `C:` 规范化为 `C:\`
        let r2 = stat_path("C:").unwrap();
        assert_eq!(r2.path, "C:\\");
        assert_eq!(r2, r);
    }
    #[cfg(unix)]
    {
        let r = stat_path("/").unwrap();
        assert!(r.is_dir);
        assert_eq!(r.name, "/");
        assert_eq!(r.path, "/");
        assert_eq!(r.size, 0);
    }
}

// ---------- 根卷列表("") ----------

#[test]
fn root_listing_per_platform() {
    #[cfg(windows)]
    {
        let out = list_dir("").unwrap();
        assert!(!out.truncated);
        // 至少含系统盘(读环境变量兜底,本机/CI 恒为 C:)
        let sys = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
        let want_root = format!("{sys}\\");
        let e = out
            .entries
            .iter()
            .find(|e| e.path == want_root)
            .unwrap_or_else(|| panic!("system drive {want_root} must be listed"));
        assert_eq!(e.name, sys);
        assert!(e.is_dir);
        assert_eq!(e.size, 0);
        // 每个根卷条目都是可再入的绝对盘根路径
        for e in &out.entries {
            let b = e.path.as_bytes();
            assert_eq!(b.len(), 3, "root path form: {}", e.path);
            assert!(b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\');
            stat_path(&e.path).unwrap();
        }
    }
    #[cfg(unix)]
    {
        let out = list_dir("").unwrap();
        assert_eq!(out.entries.len(), 1);
        assert_eq!(
            out.entries[0],
            DirEntryData {
                name: "/".to_string(),
                path: "/".to_string(),
                size: 0,
                mtime_ms: 0,
                is_dir: true,
            }
        );
    }
}

// ---------- 错误映射 ----------

#[test]
fn errors_not_found_invalid_path_and_codes() {
    let (dir, _, _) = seed_mixed();
    let base = dir.path().to_str().unwrap();

    // 父目录在、子路径缺 → NOT_FOUND
    let missing = join(base, "nope.txt");
    assert_eq!(list_dir(&missing), Err(FsError::NotFound(missing.clone())));
    assert_eq!(stat_path(&missing), Err(FsError::NotFound(missing.clone())));

    // 相对路径 → INVALID_PATH
    assert_eq!(
        list_dir("relative/dir"),
        Err(FsError::InvalidPath("relative/dir".to_string()))
    );
    assert_eq!(stat_path("relative"), Err(FsError::InvalidPath("relative".to_string())));

    // 内嵌 NUL → INVALID_PATH(用绝对形态,确保是 NUL 检查命中的判定)
    #[cfg(windows)]
    let nul = "C:\\a\0b".to_string();
    #[cfg(unix)]
    let nul = "/a\0b".to_string();
    assert_eq!(list_dir(&nul), Err(FsError::InvalidPath(nul.clone())));
    assert_eq!(stat_path(&nul), Err(FsError::InvalidPath(nul.clone())));

    // stat("") → INVALID_PATH(list("") 是根卷,合法)
    assert_eq!(stat_path(""), Err(FsError::InvalidPath(String::new())));

    // 错误码映射(ctrl 通道直接透传)
    assert_eq!(FsError::NotFound("x".to_string()).code(), "NOT_FOUND");
    assert_eq!(
        FsError::PermissionDenied("x".to_string()).code(),
        "PERMISSION_DENIED"
    );
    assert_eq!(FsError::InvalidPath("x".to_string()).code(), "INVALID_PATH");
    assert_eq!(FsError::Internal("x".to_string()).code(), "INTERNAL");
}

// ---------- Unix 专属:链接不跟随 / 权限 ----------

#[cfg(unix)]
mod unix_only {
    use super::*;

    #[test]
    fn symlink_entries_do_not_follow() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("real.txt"), b"data").unwrap();
        std::os::unix::fs::symlink("sub", dir.path().join("lnk")).unwrap();
        std::os::unix::fs::symlink("real.txt", dir.path().join("flnk")).unwrap();
        let out = list_dir(dir.path().to_str().unwrap()).unwrap();
        // 目录符号链接显示为文件(V1 简化,注释记录于 fs.rs)
        let dl = find(&out.entries, "lnk");
        assert!(!dl.is_dir, "目录链接按不跟随 → is_dir=false");
        assert_eq!(dl.size, 0, "链接自身不跟随,size 为链接而非目标");
        let fl = find(&out.entries, "flnk");
        assert!(!fl.is_dir);
        // 同一路径 stat 判定一致
        assert_eq!(stat_path(&dl.path).unwrap().is_dir, false);
    }

    #[test]
    fn chmod_000_dir_is_permission_denied() {
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let res = list_dir(locked.to_str().unwrap());
        // 先还原权限,保证 tempdir 清理不受影响
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if res.is_ok() {
            // 运行于 root(权限检查被内核旁路):自跳过,不算失败
            eprintln!("skip chmod-000 assertion: running as privileged user");
            return;
        }
        assert_eq!(
            res,
            Err(FsError::PermissionDenied(locked.to_str().unwrap().to_string()))
        );
    }
}
