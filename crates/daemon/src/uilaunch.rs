//! UI 可执行文件发现与主窗唤起(Task 9):托盘「打开主窗口」与未登录
//! 点「登录」共用的唤起入口。跨平台编译(纯文件查找 + 子进程拉起,无
//! 平台专属依赖),仅 Windows 托盘线程消费,非 Windows 构建零负担。
//!
//! 解析序镜像 `mtdss_ipc::find_daemon_bin`:
//! 1. 环境变量 `MTDSS_UI_BIN`(显式覆盖,原样采用,不验证存在性);
//! 2. 当前可执行文件同目录的 `ui[.exe]`(发布布局:mtdssd 与 ui 同目录分发);
//! 3. debug 构建下从本 crate 的 `CARGO_MANIFEST_DIR` 逐级向上找
//!    `ui/src-tauri/target/{debug,release}/ui[.exe]`(cargo run / 测试场景;
//!    daemon 由工作区构建进根 target/,ui 独立 crate 自带 src-tauri/target/,
//!    故兜底路径形态与 find_daemon_bin 不同)。
//!
//! 可注入测试面:`find_ui_bin_impl` 把 env 值与 exe 路径参数化,规避测试
//! 进程内 `std::env::set_var` 的并行竞态;`tray_logic.rs` 有三路单测。

/// UI 可执行文件名(Windows 带后缀)。
fn ui_bin_name() -> &'static str {
    if cfg!(windows) {
        "ui.exe"
    } else {
        "ui"
    }
}

/// `find_ui_bin` 的可注入实现(测试面):env 值与当前 exe 路径参数化。
/// 解析序:env 原样采用 → exe 同目录 `ui[.exe]` → debug 下清单目录逐级
/// 向上(清单目录进程级不可注入,兜底路以「优雅返回」口径测)。
pub fn find_ui_bin_impl(
    env: Option<String>,
    exe: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    if let Some(p) = env {
        return Some(std::path::PathBuf::from(p));
    }
    let file_name = ui_bin_name();
    if let Some(dir) = exe.as_deref().and_then(std::path::Path::parent) {
        let cand = dir.join(file_name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    #[cfg(debug_assertions)]
    {
        if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
            for anc in std::path::PathBuf::from(manifest).ancestors() {
                for profile in ["debug", "release"] {
                    let cand = anc
                        .join("ui")
                        .join("src-tauri")
                        .join("target")
                        .join(profile)
                        .join(file_name);
                    if cand.is_file() {
                        return Some(cand);
                    }
                }
            }
        }
    }
    None
}

/// 解析 ui 可执行文件路径(解析序见模块文档)。
pub fn find_ui_bin() -> Option<std::path::PathBuf> {
    find_ui_bin_impl(
        std::env::var("MTDSS_UI_BIN").ok(),
        std::env::current_exe().ok(),
    )
}

/// 拉起主窗:找到 ui 即后台 spawn(不等子进程,生命周期归其自身;
/// stdout/stderr 置空——子进程与托盘线程无输出关系)。Windows 加
/// CREATE_NO_WINDOW:debug 构建的 ui 是控制台子系统(release 经
/// windows_subsystem = "windows" 已是 GUI 子系统),不加会从托盘拉起时
/// 闪一个新控制台窗;加了对 release 无害,对齐 cli 拉起 mtdssd 的写法。
/// 找不到或拉起失败只 warn 留痕,不 panic——托盘线程 panic 即静默死亡,
/// 唤起失败绝不容异常上抛。
pub fn open_main_window() {
    let Some(bin) = find_ui_bin() else {
        tracing::warn!(
            "未找到 ui 可执行文件(MTDSS_UI_BIN 未设置且部署/调试路径均未命中),无法打开主窗口"
        );
        return;
    };
    let mut cmd = std::process::Command::new(&bin);
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    if let Err(err) = cmd.spawn() {
        tracing::warn!(%err, bin = %bin.display(), "拉起主窗口失败");
    }
}
