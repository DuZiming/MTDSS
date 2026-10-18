//! settings.json:守护进程持久化设置(home 下 `settings.json`)。
//!
//! 字段消费:recv_dir 由 set_recv_dir/常驻接收使用,autostart 由
//! `autostart.set` RPC 与托盘「开机自启」经 state.set_autostart 接线
//! (Task 9:Windows 同步落 HKCU Run 注册表)。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::session::write_private;

/// settings.json 内容:字段缺失或整个文件缺失均回退默认(serde default;
/// Default 派生即默认:autostart=false、recv_dir=None)。
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(default)]
pub struct Settings {
    /// 开机自启(Windows 上与 HKCU Run 的 MTDSS 值同步)。
    pub autostart: bool,
    /// 接收目录覆盖;None = 使用 `{home}/inbox`。存字符串便于手改。
    pub recv_dir: Option<String>,
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
