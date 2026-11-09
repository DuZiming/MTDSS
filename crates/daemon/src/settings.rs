//! settings.json:守护进程持久化设置(home 下 `settings.json`)。
//!
//! 字段消费:recv_dir 由 set_recv_dir/常驻接收使用,autostart 由
//! `autostart.set` RPC 与托盘「开机自启」经 state.set_autostart 接线
//! (Task 9:Windows 同步落 HKCU Run 注册表);update_auto_check/
//! update_auto_install 为更新器偏好(M4b Task 10,`update.prefs.*` RPC
//! 读写,Task 11 消费)。V3-4 §5 起 Windows 设置面四项:allow_remote_delete
//! (远端删除门,§5-1)/ show_hidden(显示隐藏项,§5-3)/ trust_mode +
//! trusted_roots(可选信任目录模式,§5-2:开 = 启动授权根 收件箱+所选
//! 目录,运行时切换经 roots.set 热注册免重启)/ device_name(设备名,
//! §5-4:从登录表单迁入,改名下次登录生效)。五字段缺省 = 旧行为
//! 零回归(门全关/信任直通/无设备名),消费面:`settings.get`/`settings.set`
//! RPC(api.rs)与 main.rs 启动信任段读取。

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
    /// 远端删除门(V3-4 §4.2/§5-1,默认关):关 = 其他设备不可删除本机
    /// 文件(UI 入口收口;引擎 ctrl delete 契约不变,本地删除不设门)。
    #[serde(default)]
    pub allow_remote_delete: bool,
    /// 显示隐藏项(V3-4 §4.5/§5-3,默认关):关 = UI 层滤除 dotfile
    /// (daemon list 不滤,iOS 同口径);开 = 全量透出。
    #[serde(default)]
    pub show_hidden: bool,
    /// 可选信任目录模式(V3-4 §5-2,默认关 = 全盘直通零回归):开 =
    /// 授权根收紧为 收件箱 + trusted_roots(启动经 main.rs 注入;运行时
    /// 切换经 roots.set 热注册)。
    #[serde(default)]
    pub trust_mode: bool,
    /// 信任目录表(trust_mode 开时的授权根增补;绝对路径串,顺序保留;
    /// 非法路径由守卫构造期丢弃 fail-closed)。
    #[serde(default)]
    pub trusted_roots: Vec<String>,
    /// 设备名覆盖(V3-4 §5-4,默认 None = 登录回落主机名):登录表单
    /// 迁入设置面,改名下次登录生效(既有语义)。
    #[serde(default)]
    pub device_name: Option<String>,
}

impl Default for Settings {
    /// D8 默认面 + V3-4 五新字段旧行为缺省(见类型注释);serde 的容器/
    /// 字段缺省同源于此。
    fn default() -> Self {
        Self {
            autostart: false,
            recv_dir: None,
            update_auto_check: true,
            update_auto_install: false,
            allow_remote_delete: false,
            show_hidden: false,
            trust_mode: false,
            trusted_roots: Vec::new(),
            device_name: None,
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

    /// V3-4 §5(Windows 设置面):五新字段缺省 = 旧行为零回归——删除门
    /// 关、显示隐藏关、信任模式关(= 启动授权根 None 直通)、信任目录
    /// 空表、设备名 None(登录回落主机名)。老 settings.json(无新字段)
    /// 与全新 home(文件缺失)两条路径同口径。
    #[test]
    fn v34_new_fields_default_to_legacy_behavior() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), r#"{"autostart":true}"#).unwrap();
        let s = Settings::load(dir.path()).unwrap();
        assert!(!s.allow_remote_delete, "老 settings.json → 删除门默认关");
        assert!(!s.show_hidden, "老 settings.json → 显示隐藏默认关");
        assert!(!s.trust_mode, "老 settings.json → 信任模式默认关(直通)");
        assert!(s.trusted_roots.is_empty(), "老 settings.json → 信任目录空表");
        assert_eq!(s.device_name, None, "老 settings.json → 无设备名");

        let fresh = tempfile::tempdir().unwrap();
        let s2 = Settings::load(fresh.path()).unwrap();
        assert!(!s2.allow_remote_delete, "文件缺失 → 删除门默认关");
        assert!(!s2.show_hidden, "文件缺失 → 显示隐藏默认关");
        assert!(!s2.trust_mode, "文件缺失 → 信任模式默认关(直通)");
        assert!(s2.trusted_roots.is_empty(), "文件缺失 → 信任目录空表");
        assert_eq!(s2.device_name, None, "文件缺失 → 无设备名");
    }

    /// V3-4 §5:五新字段写读回环(save→load 同值),且既有字段不受影响
    /// ——设置面持久化走同一 settings.json,不另立存储。
    #[test]
    fn v34_fields_roundtrip_through_settings_json() {
        let dir = tempfile::tempdir().unwrap();
        let s = Settings {
            autostart: true,
            recv_dir: Some(r"C:\_recv".into()),
            update_auto_check: false,
            update_auto_install: false,
            allow_remote_delete: true,
            show_hidden: true,
            trust_mode: true,
            trusted_roots: vec![r"D:\媒体库".into(), r"C:\代码".into()],
            device_name: Some("台式机".into()),
        };
        s.save(dir.path()).unwrap();
        let back = Settings::load(dir.path()).unwrap();
        assert!(back.allow_remote_delete);
        assert!(back.show_hidden);
        assert!(back.trust_mode);
        assert_eq!(back.trusted_roots, vec![r"D:\媒体库".to_string(), r"C:\代码".to_string()]);
        assert_eq!(back.device_name.as_deref(), Some("台式机"));
        assert!(back.autostart, "既有字段不受影响");
        assert_eq!(back.recv_dir.as_deref(), Some(r"C:\_recv"));
    }
}
