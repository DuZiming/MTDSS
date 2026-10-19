//! 远程浏览 FS 服务(`core::fs`):根卷/列表/stat/保留名过滤/截断;
//! 操作面(M3c):delete(回收优先)/move(rename+跨卷回退)/copy(暂存落位)。
//!
//! 全部走真实临时目录(tempfile):混合文件/目录与 `.mtdss`/`.mtdss-part`
//! 保留名滤除、目录优先排序、mtime/size 字段、`path` 字段可再入、
//! 截断注入 cap、各平台根卷形态、错误码映射;Unix 侧另有符号链接
//! 不跟随与 chmod 000 → `PermissionDenied`(运行于 root 时自跳过)。
//! 操作面真删真拷真移(回收路径真调 trash;跨卷回退经注入 rename 报
//! EXDEV 驱动全链路;「无回收」经 `delete_paths_inner` 注入面测)。

use mtdss_core::fs::{
    copy_paths, delete_paths, delete_paths_inner, list_dir, list_dir_capped, move_entries,
    move_entries_inner, stat_path, DirEntryData, FsError, LIST_ENTRY_CAP,
};
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
    // 7 条 cap=5 → 恰 5 条 + truncated,且保留的是排序后的前 5 条
    // (确定子集,而非 OS 读取顺序的任意切片)
    let capped = list_dir_capped(p, 5).unwrap();
    assert_eq!(capped.entries.len(), 5);
    assert!(capped.truncated);
    let names: Vec<&str> = capped.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["f0.txt", "f1.txt", "f2.txt", "f3.txt", "f4.txt"]);
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

// ---------- 保留名:大小写不敏感 ----------

#[test]
fn list_filters_reserved_names_case_insensitive() {
    let dir = tempfile::tempdir().unwrap();
    // 大小写变体(目录 .MTDSS、文件 .Mtdss-part)与 .mtdss/.mtdss-part
    // 同被滤除——Windows 大小写不敏感文件系统上同名变体必须视为同一保留名
    std::fs::create_dir(dir.path().join(".MTDSS")).unwrap();
    std::fs::write(dir.path().join(".Mtdss-part"), b"junk").unwrap();
    // 精确匹配语义保留:后缀形 `x.mtdss-part`(非保留名本体)不误伤,
    // 防实现退化成 contains 式误杀
    std::fs::write(dir.path().join("x.mtdss-part"), b"keep").unwrap();
    std::fs::write(dir.path().join("keep.txt"), b"k").unwrap();
    let out = list_dir(dir.path().to_str().unwrap()).unwrap();
    let names: Vec<&str> = out.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["keep.txt", "x.mtdss-part"]);
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

// ---------- Windows 专属:盘符相对路径 ----------

#[cfg(windows)]
#[test]
fn drive_relative_path_rejected() {
    // `C:foo` 是「C 盘当前目录下的 foo」盘符相对形态——语义随进程当前目录
    // 漂移,非用户所见形态,两入口(list/stat)一律 InvalidPath
    assert_eq!(
        list_dir("C:foo"),
        Err(FsError::InvalidPath("C:foo".to_string()))
    );
    assert_eq!(
        stat_path("C:foo"),
        Err(FsError::InvalidPath("C:foo".to_string()))
    );
    // 收紧不误伤:裸盘符 `C:`(规范化 `C:\`)、`C:\` 与第三字符为 `/` 的
    // `C:/` 语义不变
    stat_path("C:").unwrap();
    stat_path("C:\\").unwrap();
    stat_path("C:/").unwrap();
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
    use std::os::unix::fs::PermissionsExt;

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
        // 链接自身不跟随:size 是链接项的 lstat 值(POSIX 上 = 目标路径
        // 串长度,即 "sub".len() == 3),不是目标的 size
        assert_eq!(dl.size, "sub".len() as u64);
        let fl = find(&out.entries, "flnk");
        assert!(!fl.is_dir);
        // 同一路径 stat 判定一致
        assert!(!stat_path(&dl.path).unwrap().is_dir);
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

// ---------- 操作面:删除 / 移动 / 本地复制(M3c) ----------

/// 嵌套目录树样例:`root/<name>/{f0.txt, d1/f1.txt, d1/d2/f2.txt}`。
fn seed_tree(root: &Path, name: &str) {
    let t = root.join(name);
    std::fs::create_dir_all(t.join("d1").join("d2")).unwrap();
    std::fs::write(t.join("f0.txt"), b"0").unwrap();
    std::fs::write(t.join("d1").join("f1.txt"), b"1").unwrap();
    std::fs::write(t.join("d1").join("d2").join("f2.txt"), b"22").unwrap();
}

fn exists(p: &Path) -> bool {
    p.symlink_metadata().is_ok()
}

/// 目录在场且无任何条目(暂存清理断言用)。
fn dir_is_empty(p: &Path) -> bool {
    std::fs::read_dir(p).expect("dir present").next().is_none()
}

#[test]
fn delete_force_permanent_removes_file_and_tree() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().to_str().unwrap().to_string();
    std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
    seed_tree(dir.path(), "tree");
    let (n, recycled, failed) =
        delete_paths(&[join(&base, "a.txt"), join(&base, "tree")], true).unwrap();
    assert_eq!(n, 2, "两条全成功");
    assert!(!recycled, "force=true 不走回收");
    assert!(failed.is_empty(), "无失败项: {failed:?}");
    assert!(!exists(&dir.path().join("a.txt")));
    assert!(!exists(&dir.path().join("tree")), "目录整树消失");
}

/// 回收路径真调 trash:断言原路径消失且整体非错误(更非 NotFound)。
/// Windows 必测(IFileOperation 恒可用);CI Linux freedesktop trash 对
/// 临时目录(可能异于 home 的挂载点)可用性不稳,失败即自跳过记 warn。
#[test]
fn delete_recycle_moves_to_trash() {
    let dir = tempdir_for_recycle();
    let f = dir.path().join("recyclable.txt");
    std::fs::write(&f, b"gone soon").unwrap();
    match delete_paths(&[f.to_string_lossy().into_owned()], false) {
        Ok((1, true, failed)) => {
            assert!(failed.is_empty(), "无失败项: {failed:?}");
            assert!(!exists(&f), "原路径必须消失(进了回收站)");
        }
        #[cfg(unix)]
        Err(e) => eprintln!(
            "skip recycle assertion: freedesktop trash unavailable on this host: {e:?}"
        ),
        #[cfg(windows)]
        Err(e) => panic!("Windows IFileOperation 回收必须可用: {e:?}"),
        Ok(other) => panic!("意外结果: {other:?}"),
    }
}

/// 回收测试用 home 挂载点内的临时目录(freedesktop home trash 直接适用;
/// /tmp 多为独立 tmpfs,回收区可用性不可控)。home 不可得回退默认临时目录。
#[cfg(unix)]
fn tempdir_for_recycle() -> tempfile::TempDir {
    if let Some(home) = std::env::var_os("HOME") {
        if !home.is_empty() {
            if let Ok(d) = tempfile::Builder::new()
                .prefix("mtdss-recycle-test")
                .tempdir_in(home)
            {
                return d;
            }
        }
    }
    tempfile::tempdir().unwrap()
}

#[cfg(windows)]
fn tempdir_for_recycle() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn delete_recycle_unavailable_injected_is_batch_error() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("x.txt");
    std::fs::write(&f, b"x").unwrap();
    let paths = vec![f.to_string_lossy().into_owned()];
    // 设施级失败注入(Unsupported = 生产 recycle_io 对设施不可用的映射,
    // freedesktop「无 home trash 也无挂载点回收区」即此形态):批级
    // RecycleUnavailable 上抛,任何一条都不许动——操作端要整体确认后
    // force 重发
    let r = delete_paths_inner(&paths, false, |_p| {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no recycle backend",
        ))
    });
    assert_eq!(
        r,
        Err(FsError::RecycleUnavailable("no recycle backend".to_string()))
    );
    // 错误码钉死(ctrl 透传 + CLI 标记串判定的依据)
    assert_eq!(
        FsError::RecycleUnavailable("x".to_string()).code(),
        "RECYCLE_UNAVAILABLE"
    );
    assert!(exists(&f), "批级失败不得动到任何一条");
}

#[test]
fn delete_recycle_item_failure_is_per_item_injected() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("locked.txt");
    std::fs::write(&f, b"busy").unwrap();
    let paths = vec![f.to_string_lossy().into_owned()];
    // 目标级失败(如被锁)≠ 设施不可用:进逐条失败表,批正常返回
    let (n, recycled, failed) = delete_paths_inner(&paths, false, |_p| {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "file in use",
        ))
    })
    .unwrap();
    assert_eq!((n, recycled), (0, false));
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].code, "PERMISSION_DENIED");
    assert_eq!(failed[0].path, f.to_string_lossy());
}

#[test]
fn delete_partial_failure_reports_per_item() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().to_str().unwrap().to_string();
    std::fs::write(dir.path().join("ok.txt"), b"k").unwrap();
    // [缺失, 正常, 形态非法] → 成功 1、失败表按序 NOT_FOUND + INVALID_PATH,
    // 部分失败不弃批
    let (n, recycled, failed) = delete_paths(
        &[
            join(&base, "nope.txt"),
            join(&base, "ok.txt"),
            "relative/x".to_string(),
        ],
        true,
    )
    .unwrap();
    assert_eq!(n, 1);
    assert!(!recycled);
    assert_eq!(failed.len(), 2);
    assert_eq!(failed[0].code, "NOT_FOUND");
    assert_eq!(failed[0].path, join(&base, "nope.txt"));
    assert_eq!(failed[1].code, "INVALID_PATH");
    assert_eq!(failed[1].path, "relative/x");
}

#[test]
fn move_same_volume_renames_file_and_tree() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().to_str().unwrap().to_string();
    std::fs::write(dir.path().join("a.txt"), b"AAA").unwrap();
    seed_tree(dir.path(), "tree");
    std::fs::create_dir(dir.path().join("mvdir")).unwrap();
    // 覆盖目标:既有文件被替换(rename 语义钉死;冲突策略判定在调用方)
    std::fs::write(dir.path().join("mvdir").join("b.txt"), b"OLD").unwrap();
    let pairs = vec![
        (join(&base, "a.txt"), join(&base, "mvdir/b.txt")),
        (join(&base, "tree"), join(&base, "mvdir/tree")),
    ];
    let (n, failed) = move_entries(&pairs).unwrap();
    assert_eq!(n, 2);
    assert!(failed.is_empty(), "无失败项: {failed:?}");
    assert!(!exists(&dir.path().join("a.txt")));
    assert!(!exists(&dir.path().join("tree")));
    assert_eq!(
        std::fs::read(dir.path().join("mvdir/b.txt")).unwrap(),
        b"AAA",
        "文件目标覆盖语义(与 rename 一致)"
    );
    assert_eq!(
        std::fs::read(dir.path().join("mvdir/tree/d1/f1.txt")).unwrap(),
        b"1",
        "整树移动内容保全"
    );
    // 同卷改名直通,不产生 .mtdss 暂存痕迹
    assert!(!exists(&dir.path().join("mvdir/.mtdss")));
}

#[test]
fn move_cross_volume_falls_back_to_staged_copy_injected() {
    // 真实第二卷/跨 tmpfs 挂载在测试环境不可行 → 注入报 EXDEV 的 rename
    // 驱动回退全链路(生产传 std::fs::rename,真实跨卷同样报 EXDEV)
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().to_str().unwrap().to_string();
    std::fs::write(dir.path().join("a.txt"), b"CROSS").unwrap();
    seed_tree(dir.path(), "tree");
    std::fs::create_dir(dir.path().join("dst")).unwrap();
    let exdev = |_f: &Path, _t: &Path| -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::CrossesDevices,
            "EXDEV (injected)",
        ))
    };
    let pairs = vec![
        (join(&base, "a.txt"), join(&base, "dst/a.txt")),
        (join(&base, "tree"), join(&base, "dst/tree")),
    ];
    let (n, failed) = move_entries_inner(&pairs, exdev).unwrap();
    assert_eq!(n, 2);
    assert!(failed.is_empty(), "无失败项: {failed:?}");
    // 源永久删除(钉死:回退复制对调用方透明,数据已在目的地)
    assert!(!exists(&dir.path().join("a.txt")));
    assert!(!exists(&dir.path().join("tree")));
    assert_eq!(std::fs::read(dir.path().join("dst/a.txt")).unwrap(), b"CROSS");
    assert_eq!(
        std::fs::read(dir.path().join("dst/tree/d1/d2/f2.txt")).unwrap(),
        b"22",
        "目录树跨卷整树保全"
    );
    // 暂存清理:目标目录 .mtdss 落空
    assert!(dir_is_empty(&dir.path().join("dst/.mtdss")));
}

#[test]
fn move_errors_reported_per_item() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().to_str().unwrap().to_string();
    std::fs::create_dir(dir.path().join("dst")).unwrap();
    // [缺失源, 非法形态] → 全失败但逐条上报,不 panic 不弃批
    let pairs = vec![
        (join(&base, "nope.txt"), join(&base, "dst/x.txt")),
        ("relative/a.txt".to_string(), join(&base, "dst/y.txt")),
    ];
    let (n, failed) = move_entries(&pairs).unwrap();
    assert_eq!(n, 0);
    assert_eq!(failed.len(), 2);
    assert_eq!(failed[0].code, "NOT_FOUND");
    assert_eq!(failed[0].path, join(&base, "nope.txt"));
    assert_eq!(failed[1].code, "INVALID_PATH");
}

#[test]
fn copy_paths_single_file_lands_and_staging_empties() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("a.txt"), b"COPY").unwrap();
    let landed = copy_paths(
        &[src.path().join("a.txt").to_string_lossy().into_owned()],
        dst.path().to_str().unwrap(),
    )
    .unwrap();
    assert_eq!(landed.len(), 1);
    assert_eq!(
        Path::new(&landed[0]),
        dst.path().join("a.txt").as_path(),
        "返回落盘路径"
    );
    assert_eq!(std::fs::read(&landed[0]).unwrap(), b"COPY", "内容一致");
    assert!(src.path().join("a.txt").exists(), "复制保留源");
    assert!(dir_is_empty(&dst.path().join(".mtdss")), "暂存键完成后清空");
}

#[test]
fn copy_paths_nested_tree_and_collision_auto_rename() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    seed_tree(src.path(), "tree");
    let srcs = src.path().to_string_lossy().into_owned();
    let dsts = dst.path().to_str().unwrap().to_string();
    let first = copy_paths(&[join(&srcs, "tree")], &dsts).unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(
        std::fs::read(dst.path().join("tree/d1/d2/f2.txt")).unwrap(),
        b"22",
        "整树复制内容保全"
    );
    // 目标同名 → unique_target 语义自动改名 `tree (1)`(冲突策略判定在调用方)
    let second = copy_paths(&[join(&srcs, "tree")], &dsts).unwrap();
    assert_eq!(Path::new(&second[0]), dst.path().join("tree (1)").as_path());
    assert_eq!(
        std::fs::read(dst.path().join("tree (1)/d1/f1.txt")).unwrap(),
        b"1"
    );
    assert!(dir_is_empty(&dst.path().join(".mtdss")), "两次复制后暂存仍落空");
}

#[test]
fn copy_paths_rejects_missing_source_and_non_dir_target() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let srcs = src.path().to_string_lossy().into_owned();
    let dsts = dst.path().to_str().unwrap().to_string();
    // 缺失源 → NotFound(整批拒,目标目录无痕)
    let r = copy_paths(&[join(&srcs, "nope.txt")], &dsts);
    assert_eq!(r, Err(FsError::NotFound(join(&srcs, "nope.txt"))));
    // 目标是文件 → Internal(入口即拒)
    std::fs::write(dst.path().join("f.txt"), b"f").unwrap();
    std::fs::write(src.path().join("x.txt"), b"x").unwrap();
    let r = copy_paths(
        &[join(&srcs, "x.txt")],
        dst.path().join("f.txt").to_str().unwrap(),
    );
    assert!(matches!(r, Err(FsError::Internal(_))), "目标非目录: {r:?}");
    // 无名形态(卷根)不可复制 → InvalidPath
    #[cfg(windows)]
    {
        let r2 = copy_paths(&["C:\\".to_string()], &dsts);
        assert_eq!(r2, Err(FsError::InvalidPath("C:\\".to_string())));
    }
    #[cfg(unix)]
    {
        let r2 = copy_paths(&["/".to_string()], &dsts);
        assert_eq!(r2, Err(FsError::InvalidPath("/".to_string())));
    }
}
