//! 冲突预扫与本地源展开的纯函数面(M3c Task 6)。
//!
//! 操作端冲突决策的数据面:传输前把源侧条目与目标目录列表比对,同名双
//! 文件 / 同名类型不匹配即冲突(**目录对目录恒合并**,create_dir_all 语义,
//! 与引擎 overwrite 不清目录占用、自动改名安全网兜底嵌套同名的口径一致);
//! UI/CLI 的冲突对话框据此问询,答出的 skip/overwrite 表再随定向传输请求
//! 下发(Task 5 信令面)。本模块不触碰网络与磁盘写——全部纯函数;源展开
//! 沿 collect_items 的 rel 口径但只读元数据,错误一律「跳过并告警」,绝不
//! 把预扫放大为传输失败(预扫时刻的磁盘事实本就可能与传输时刻漂移)。

use std::path::{Path, PathBuf};

/// 预扫出的冲突条目:UI/CLI 问询行(src 大小 vs dst 大小/修改时间)与
/// 策略表生成的数据源(Task 7/11 消费,字段钉死)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// 源侧 rel(全树口径;顶层输入粒度时即输入名)。
    pub rel: String,
    /// 源侧是否目录(类型不匹配展示用)。
    pub src_is_dir: bool,
    /// 源侧字节数(目录为 0:目录大小对冲突判定无意义)。
    pub src_size: u64,
    /// 目标侧字节数(目录为 0)。
    pub dst_size: u64,
    /// 目标侧修改时间(毫秒;问询行「目标修改时间」列)。
    pub dst_mtime_ms: u64,
}

/// 目标侧条目与目标目录列表的冲突比对(纯函数)。
///
/// 比对按**名字相等**:源 rel(顶层粒度或本地全树 rel)对目标条目名。
/// 分类:
/// - 同名双双为**文件** → 冲突(同名内容之争,须用户/策略裁决);
/// - 同名**类型不匹配**(文件↔目录,双向)→ 冲突(引擎 overwrite 只清
///   文件型占用,目录占用走自动改名安全网——预扫如实报冲突,由上层
///   裁决,不静默合并);
/// - 同名**目录对目录** → 无冲突(目录恒合并);
/// - 无同名 → 无冲突。
///
/// 目标侧粒度由调用方决定:顶层列表(嵌套同名交给引擎安全网)或全树
/// rel(全粒度预扫)。输出按源序稳定;不排序不改写,纯投影。
pub fn scan_conflicts(
    sources: &[(String, bool, u64)],
    dst_entries: &[(String, bool, u64, u64)],
) -> Vec<Conflict> {
    sources
        .iter()
        .filter_map(|(rel, src_is_dir, src_size)| {
            // 名字相等即比对:目标侧无论哪种粒度,条目名与源 rel 同一命名空间
            let d = dst_entries.iter().find(|(name, _, _, _)| name == rel)?;
            let (dst_is_dir, dst_size, dst_mtime_ms) = (d.1, d.2, d.3);
            let conflict = (*src_is_dir != dst_is_dir) || (!*src_is_dir && !dst_is_dir);
            // 文件对文件 → 冲突;类型不匹配 → 冲突;目录对目录 → 恒合并(无)
            conflict.then(|| Conflict {
                rel: rel.clone(),
                src_is_dir: *src_is_dir,
                src_size: *src_size,
                dst_size,
                dst_mtime_ms,
            })
        })
        .collect()
}

/// 本地源展开为全树 rel(供 upload 全粒度预扫):沿 collect_items 口径
/// (文件 → file_name;目录 → 递归展开,rel = `输入目录名/内部路径`,
/// 同目录按名排序),但**只读元数据**:缺失/不可读输入跳过并告警(不
/// Err),符号链接跳过,`.mtdss` 保留段(大小写不敏感)整段跳过。
/// 目录条目本身入表(is_dir=true, size=0),先于其子项(DFS 先序)。
pub fn local_tree_rels(paths: &[PathBuf]) -> Vec<(String, bool, u64)> {
    let mut out: Vec<(String, bool, u64)> = Vec::new();
    for input in paths {
        let Ok(meta) = std::fs::symlink_metadata(input) else {
            tracing::warn!("预扫跳过不可读输入 {}", input.display());
            continue;
        };
        if meta.is_symlink() {
            tracing::warn!("预扫跳过符号链接输入 {}", input.display());
            continue;
        }
        let Some(base) = input.file_name().and_then(|n| n.to_str()) else {
            tracing::warn!("预扫跳过非 UTF-8 文件名输入 {}", input.display());
            continue;
        };
        if base.eq_ignore_ascii_case(".mtdss") {
            tracing::warn!("预扫跳过保留名输入 {base}(接收方 intake 区)");
            continue;
        }
        if meta.is_dir() {
            walk_tree(input, base, &mut out);
        } else if meta.is_file() {
            out.push((base.to_string(), false, meta.len()));
        } else {
            tracing::warn!("预扫跳过非常规输入 {}", input.display());
        }
    }
    out
}

/// DFS 先序展开目录:目录自身先入表,再按名排序展开子项;读失败/竞态
/// 消失的条目跳过并告警(只读预扫,不放大为失败)。
fn walk_tree(dir: &Path, prefix: &str, out: &mut Vec<(String, bool, u64)>) {
    out.push((prefix.to_string(), true, 0));
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!("预扫跳过不可读目录 {}({err})", dir.display());
            return;
        }
    };
    let mut entries: Vec<std::fs::DirEntry> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => {
                tracing::warn!("预扫跳过非 UTF-8 条目 {}", entry.path().display());
                continue;
            }
        };
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue; // 竞态消失:跳过
        };
        if meta.is_symlink() {
            tracing::warn!("预扫跳过符号链接 {}", path.display());
            continue;
        }
        if name.eq_ignore_ascii_case(".mtdss") {
            tracing::warn!("预扫跳过保留段条目 {}", path.display());
            continue;
        }
        let rel = format!("{prefix}/{name}");
        if meta.is_dir() {
            walk_tree(&path, &rel, out);
        } else {
            out.push((rel, false, meta.len()));
        }
    }
}

/// 采样 EMA 速度(纯函数,字节/秒)。
///
/// `samples` 为 `(毫秒时刻, 累计字节)` 单调序列(作业进度回调侧采样);
/// 只统计与最新样本时差 ≤ `window_ms` 的窗口内样本,相邻对求瞬时速率后
/// EMA 平滑(α = 2/(n+1),n = 窗口内速率样本数)——既压毛刺又不至于像
/// 全窗均值那样对骤停迟钝。四舍五入取整:恒速输入得到精确速率(截断会
/// 把 0.666…/0.333… 的浮点尾差吃成低位偏差)。样本不足(<2)或全为
/// dt=0 的并排对 → 0。
///
/// brief 钉在 jobs.rs 的 EMA 纯函数;落位本纯函数模块以对 ops_unit 集成
/// 测试可见(jobs 内 pub(crate) 集成测试不可达),签名逐字一致。
pub fn ema_speed(samples: &[(u64, u64)], window_ms: u64) -> u64 {
    if samples.len() < 2 {
        return 0;
    }
    let last_ms = samples[samples.len() - 1].0;
    let win: Vec<(u64, u64)> = samples
        .iter()
        .filter(|(ms, _)| last_ms.saturating_sub(*ms) <= window_ms)
        .copied()
        .collect();
    if win.len() < 2 {
        return 0;
    }
    let n = (win.len() - 1) as f64;
    let alpha = 2.0 / (n + 1.0);
    let mut ema: Option<f64> = None;
    for pair in win.windows(2) {
        let (ms0, b0) = pair[0];
        let (ms1, b1) = pair[1];
        let dt = ms1.saturating_sub(ms0);
        if dt == 0 {
            continue; // 并排采样(同毫秒多回调):无时间差即无速率
        }
        let rate = b1.saturating_sub(b0) as f64 * 1000.0 / dt as f64;
        ema = Some(match ema {
            // 首个速率直接作为 EMA 初值(EMA of 常数 = 常数,恒速精确)
            None => rate,
            Some(prev) => alpha * rate + (1.0 - alpha) * prev,
        });
    }
    ema.map(|v| v.round() as u64).unwrap_or(0)
}
