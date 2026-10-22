//! settings.json:守护进程持久化设置(home 下 `settings.json`)。
//!
//! 字段消费:recv_dir 由 set_recv_dir/常驻接收使用,autostart 由
//! `autostart.set` RPC 与托盘「开机自启」经 state.set_autostart 接线
//! (Task 9:Windows 同步落 HKCU Run 注册表);update_auto_check/
//! update_auto_install 为更新器偏好(M4b Task 10,`update.prefs.*` RPC
//! 读写,Task 11 消费)。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::session::write_private;

/// update_auto_check 的 serde 字段缺省(D8:自动检查默认开)。
fn default_true() -> bool {
    true
}

/// settings.json 内容:字段缺失或整个文件缺失均回退默认(serde default;
/// Default 手写即 D8 默认面:autostart=false、recv_dir=None、
/// update_auto_check=true(检查默认开)、update_auto_install=false(安装
/// 默认关)。文件缺失与字段缺失同口径——两条路径都从本实现取值,默认
/// 只写一处,不因派生 Default 的 bool 零值把检查默认翻成关)。
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct Settings {
    /// 开机自启(Windows 上与 HKCU Run 的 MTDSS 值同步)。
    pub autostart: bool,
    /// 接收目录覆盖;None = 使用 `{home}/inbox`。存字符串便于手改。
    pub recv_dir: Option<String>,
    /// 自动检查更新(D8 默认开;D4 父子耦合:关闭即连带关自动安装,
    /// 收口在 DaemonState::set_update_check 一处)。
    #[serde(default = "default_true")]
    pub update_auto_check: bool,
    /// 自动安装更新(D8 默认关;仅在 auto_check 开启时有意义)。
    #[serde(default)]
    pub update_auto_install: bool,
}

impl Default for Settings {
    /// D8 默认面(见类型注释);serde 的容器/字段缺省同源于此。
    fn default() -> Self {
        Self {
            autostart: false,
            recv_dir: None,
            update_auto_check: true,
            update_auto_install: false,
        }
    }
}

impl Settings {
    /// 从 home 读取 settings.json;文件缺失即默认,存在但损坏则硬错误。
    pub fn load(home: &Path) -> Result<Self> {
        let p = home.join("settings.json");
        if !p.exists() {
            return Ok(Self::default());
        }
        let s = std::fs::read_to_string(&p)?;
        Ok(serde_json::from_str(&s)?)
    }

    /// 写回 home/settings.json(与 session 同口径的私有落盘)。
    pub fn save(&self, home: &Path) -> Result<()> {
        write_private(
            &home.join("settings.json"),
            &serde_json::to_string_pretty(self)?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M4b Task 10(D8 默认):老 settings.json(无新字段)加载 →
    /// auto_check=true / auto_install=false;全新 home(文件缺失)同样
    /// 出 D8 默认(Default 手写后文件缺失与字段缺失同口径,不因派生
    /// Default 的 bool 零值把检查默认翻成关)。
    #[test]
    fn old_or_missing_settings_json_gets_d8_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), r#"{"autostart":true}"#).unwrap();
        let s = Settings::load(dir.path()).unwrap();
        assert!(s.autostart, "老字段照常读出");
        assert!(s.update_auto_check, "无新字段 → D8 默认检查开");
        assert!(!s.update_auto_install, "无新字段 → D8 默认安装关");

        let fresh = tempfile::tempdir().unwrap();
        let s2 = Settings::load(fresh.path()).unwrap();
        assert!(s2.update_auto_check, "文件缺失 → D8 默认检查开");
        assert!(!s2.update_auto_install, "文件缺失 → D8 默认安装关");
    }
}
