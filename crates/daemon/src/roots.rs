//! 授权根守卫(v2-P5 T2;spec §3「授权目录集」的引擎侧正身)。
//!
//! iOS 形态(spec §3/§5.1):授权集 = 收件箱(App 初始授权,恒在)+ 信任
//! 目录(用户经设置添加,T10 书签解析结果经 `DaemonOpts::authorized_roots`
//! 注入)。授权集内引擎可读 → ①其他设备可浏览进来(ctrl 面的 List/Stat 及
//! 既有 delete/move/copy 等本机路径操作)②可作 App 内发送源(jobs 面的
//! upload 源路径,含编排回显与重发)。集外路径一律拒。
//!
//! 桌面默认 `None` = 未配置 = 守卫直通,**行为逐字节不变**(全局约束:
//! 桌面 bin 行为不变;本模块只在 Some 形态下收口)。
//!
//! 手法与 Windows put「平台无关绝对形态预检」同源(flow::remote_absolute_form
//! 先例):先过 NUL + 本平台绝对形态关(`mtdss_core::fs::normalize`——守卫
//! 的是**本机**路径面,平台规则即本机规则),再**词法规范化**(逐组件消解
//! `.`/`..`,`..` 越出根即拒),最后按组件做授权前缀判定(`Path::components`
//! 语义,杜绝 `/a/bc` 字节前缀误判 `/a/b` 的经典坑)。纯字符串/组件操作,
//! 不 canonicalize:授权根与受检路径都可能尚不存在(发送预扫先于落盘),
//! 且词法判定与既有预检口径一致。
//!
//! 已知边界(如实记录,非本任务扩张面):符号链接指到集外的路径,词法判定
//! 拦不住(引擎读取时才跟随链接;iOS 沙箱内 Documents/信任目录由系统选择器
//! 授权,App 自身无法在集内植入外指链接,远端对端无法借 `..` 之外的形态触
//! 达——纵深加固归 T10 书签面落地时一并评估)。
//!
//! Windows 形态说明:授权集生产上只在 iOS(UNIX 路径)注入;Windows 组件
//! 比较做 ASCII 大小写字 fold(NTFS 大小写不敏感),盘符同折,防御性覆盖
//! 测试机自注入形态。

use std::path::{Path, PathBuf};

/// 授权根守卫:构造时规范化授权根(非法根丢弃留痕,fail-closed——坏根缩小
/// 授权集而非放大),`check` 做单路径判定。
#[derive(Debug, Clone)]
pub(crate) struct RootGuard {
    /// None = 未配置(桌面默认):一切路径直通。Some = 规范化后的授权根集。
    roots: Option<Vec<PathBuf>>,
}

impl RootGuard {
    /// 由注入面构造(iOS 形态:收件箱 + 信任目录;桌面 None)。
    pub(crate) fn new(roots: Option<Vec<PathBuf>>) -> Self {
        let _ = roots;
        // TDD 红桩:直通形态(实现于绿步;全部拒绝类测试在本形态下必红)。
        Self { roots: None }
    }

    /// 是否启用(未配置 = false = 直通;诊断/测试面)。
    #[allow(dead_code)] // 绿步接线后消费;红桩期保留诊断面
    pub(crate) fn enabled(&self) -> bool {
        self.roots.is_some()
    }

    /// 单路径守卫:`Some(roots)` 时路径须规范化后落在任一授权根内(含根
    /// 自身——收件箱根目录本身必须可浏览/可作发送源),否则 Err(业务文案,
    /// 与 ctrl INVALID_PATH / jobs 业务 Err 两个消费面同源)。
    pub(crate) fn check(&self, path: &Path) -> Result<(), String> {
        let _ = path;
        // TDD 红桩:直通(实现于绿步)。
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (绿步补全)授权根词法规范化与组件前缀判定的纯函数矩阵。
    #[test]
    fn placeholder_matrix_lands_with_implementation() {
        // TDD 红步占位:矩阵测试随绿步实现落地,避免红步携带假绿。
        let guard = RootGuard::new(None);
        assert!(!guard.enabled(), "未配置 = 直通形态");
    }
}
