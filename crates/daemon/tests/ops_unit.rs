//! ops 纯函数单测(M3c Task 6):冲突预扫(scan_conflicts)/ 本地源展开
//! (local_tree_rels)/ 速度 EMA(ema_speed)。
//!
//! 全部纯函数直测,不起 daemon;冲突语义钉死:同名双文件 / 同名类型不匹配
//! → 冲突,目录对目录恒合并(无冲突),目标为空永无冲突。

use mtdss_daemon::ops::{ema_speed, local_tree_rels, scan_conflicts};

/// 源侧条目 (rel, is_dir, size) 速构。
fn src(s: &str, d: bool, sz: u64) -> (String, bool, u64) {
    (s.to_string(), d, sz)
}

/// 目标侧条目 (name, is_dir, size, mtime_ms) 速构。mtime 是冲突对话框展示
/// 「目标修改时间」的数据源,必须随冲突条目穿出(见 Conflict)。
fn dst(s: &str, d: bool, sz: u64, mt: u64) -> (String, bool, u64, u64) {
    (s.to_string(), d, sz, mt)
}

#[test]
fn scan_conflicts_file_vs_file() {
    // 同名双双为文件 → 冲突(带双侧 size 与目标 mtime);无同名 → 不冲突
    let c = scan_conflicts(
        &[src("a.txt", false, 100), src("fresh.bin", false, 7)],
        &[
            dst("a.txt", false, 200, 1_234),
            dst("other.txt", false, 1, 5),
        ],
    );
    assert_eq!(c.len(), 1, "仅同名 a.txt 冲突: {c:?}");
    assert_eq!(c[0].rel, "a.txt");
    assert!(!c[0].src_is_dir, "源是文件");
    assert_eq!(c[0].src_size, 100);
    assert_eq!(c[0].dst_size, 200);
    assert_eq!(c[0].dst_mtime_ms, 1_234);
}

#[test]
fn scan_conflicts_type_mismatch() {
    // 同名类型不匹配(文件 vs 目录,两个方向)→ 都是冲突
    let c = scan_conflicts(
        &[src("x", false, 10), src("y", true, 0)],
        &[dst("x", true, 0, 9), dst("y", false, 42, 77)],
    );
    assert_eq!(c.len(), 2, "双向类型不匹配都是冲突: {c:?}");
    let x = c.iter().find(|c| c.rel == "x").expect("x 冲突");
    let y = c.iter().find(|c| c.rel == "y").expect("y 冲突");
    assert!(!x.src_is_dir, "源文件 vs 目标目录");
    assert!(y.src_is_dir, "源目录 vs 目标文件");
    assert_eq!(y.dst_size, 42);
    assert_eq!(y.dst_mtime_ms, 77);
}

#[test]
fn scan_conflicts_dir_merge_none() {
    // 目录对目录 → 无冲突(目录恒合并);嵌套 rel 在顶层粒度目标列表上
    // 无同名 → 也不冲突(嵌套同名由引擎自动改名安全网兜底,见计划)
    let c = scan_conflicts(
        &[src("proj", true, 0), src("proj/deep.txt", false, 3)],
        &[dst("proj", true, 0, 1)],
    );
    assert!(c.is_empty(), "目录对目录恒合并: {c:?}");
}

#[test]
fn scan_conflicts_empty_dst() {
    // 目标目录为空(全新落点)→ 永无冲突
    let c = scan_conflicts(&[src("a", false, 1), src("b/c", false, 2)], &[]);
    assert!(c.is_empty(), "空目标无冲突: {c:?}");
}

#[test]
fn local_tree_rels_nested_and_sorted() {
    // 混合输入:目录(嵌套)+ 单文件。口径沿 collect_items:目录条目本身
    // 入表(is_dir=true, size=0),子项 rel = `输入目录名/内部路径`,同目录
    // 按名排序;.mtdss 保留段(大小写不敏感)整段跳过;缺失输入跳过不报错
    let base = tempfile::tempdir().unwrap();
    let dir = base.path().join("树");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("b.txt"), b"bb").unwrap();
    std::fs::write(dir.join("a.txt"), b"aaa").unwrap(); // 排序在 b 前
    std::fs::write(dir.join("sub").join("c.bin"), b"cccc").unwrap();
    std::fs::create_dir_all(dir.join(".MTDSS")).unwrap();
    std::fs::write(dir.join(".MTDSS").join("x.bin"), b"x").unwrap();
    let single = base.path().join("单.bin");
    std::fs::write(&single, b"x").unwrap();

    let rels = local_tree_rels(&[dir, single]);
    let shape: Vec<(&str, bool, u64)> = rels
        .iter()
        .map(|(r, d, s)| (r.as_str(), *d, *s))
        .collect();
    assert_eq!(
        shape,
        vec![
            ("树", true, 0),
            ("树/a.txt", false, 3),
            ("树/b.txt", false, 2),
            ("树/sub", true, 0),
            ("树/sub/c.bin", false, 4),
            ("单.bin", false, 1),
        ],
        "DFS 先序(目录先于子项)+ 同目录按名排序 + 保留段跳过: {rels:?}"
    );

    // 缺失输入:跳过告警,不报错(只读预扫,不放大为失败)
    assert!(
        local_tree_rels(&[base.path().join("nope")]).is_empty(),
        "缺失输入跳过后为空表"
    );
}

#[test]
fn ema_speed_window() {
    // 恒速 1000 B/s:窗口覆盖全部样本 → EMA 恒为 1000(四舍五入后精确)
    let steady: Vec<(u64, u64)> = vec![(0, 0), (500, 500), (1000, 1000), (1500, 1500)];
    assert_eq!(ema_speed(&steady, 5_000), 1_000);

    // 窗口剔除:前段 10_000 B/s 尖峰滑出窗口后,速度只反映窗口内恒速段
    let jump: Vec<(u64, u64)> = vec![(0, 0), (1000, 10_000), (2000, 11_000), (3000, 12_000)];
    assert_eq!(ema_speed(&jump, 1_500), 1_000, "窗口外样本不得计入");
    // 同样本宽窗口:尖峰与恒速混合 → EMA 严格介于窗口内极值之间
    let mixed = ema_speed(&jump, 5_000);
    assert!(
        mixed > 1_000 && mixed < 10_000,
        "EMA 须介于窗口内极值之间: {mixed}"
    );

    // 样本不足(<2)→ 0;dt=0 的相邻对不产生除零(跳过该对)
    assert_eq!(ema_speed(&[], 1_000), 0);
    assert_eq!(ema_speed(&[(100, 55)], 1_000), 0);
    assert_eq!(ema_speed(&[(100, 5), (100, 9), (200, 209)], 1_000), 2_000);
}
