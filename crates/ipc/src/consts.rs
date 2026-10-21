//! 编译期默认端点常量:信令与更新两个**独立**来源。
//!
//! 覆盖序:运行时 env → 构建期 env(`option_env!`)→ `.invalid` 占位。
//! `.invalid` 是 RFC 2606 保留 TLD,保证未注入配置的构建绝不触碰真实端点
//! (项目反目标:测试永不触生产端点)。两个常量各自的 option_env! 键独立,
//! 注入其一不牵连另一个。

/// 构建期注入的信令服务器默认地址;未注入时落 `.invalid` 占位。
// 注:计划原文用 `option_env!(…).unwrap_or(…)`,但 `Option::unwrap_or` 尚非
// stable const fn(rust-lang/rust#143874),此处以 const 稳定的 match 等义改写
// (值与语义逐字保留)。
pub const DEFAULT_SIGNALING_URL: &str = match option_env!("MTDSS_SIGNALING_URL") {
    Some(url) => url,
    None => "https://mtdss-signaling.invalid",
};
/// 构建期注入的更新通道基址;未注入时落 `.invalid` 占位。
pub const DEFAULT_UPDATE_URL: &str = match option_env!("MTDSS_UPDATE_URL") {
    Some(url) => url,
    None => "https://mtdss-update.invalid",
};

/// 运行时解析信令地址:环境变量优先,缺省回落构建期常量。
pub fn signaling_url() -> String { std::env::var("MTDSS_SIGNALING_URL").unwrap_or_else(|_| DEFAULT_SIGNALING_URL.into()) }
/// 运行时解析更新基址:环境变量优先,缺省回落构建期常量。
pub fn update_url() -> String { std::env::var("MTDSS_UPDATE_URL").unwrap_or_else(|_| DEFAULT_UPDATE_URL.into()) }

#[cfg(test)]
mod tests {
    use super::*;

    /// 环境变量变更不可并行:持进程级锁串行化本模块全部用例。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 运行时 env 覆盖生效;remove 后回落构建期常量(信令侧)。
    #[test]
    fn signaling_url_env_overrides_then_falls_back_to_const() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("MTDSS_SIGNALING_URL", "https://sig.example:9000");
        assert_eq!(signaling_url(), "https://sig.example:9000");
        std::env::remove_var("MTDSS_SIGNALING_URL");
        assert_eq!(signaling_url(), DEFAULT_SIGNALING_URL);
    }

    /// 运行时 env 覆盖生效;remove 后回落构建期常量(更新侧)。
    #[test]
    fn update_url_env_overrides_then_falls_back_to_const() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("MTDSS_UPDATE_URL", "https://upd.example:9001");
        assert_eq!(update_url(), "https://upd.example:9001");
        std::env::remove_var("MTDSS_UPDATE_URL");
        assert_eq!(update_url(), DEFAULT_UPDATE_URL);
    }

    /// 两常量彼此独立(值不同、键不同);未注入构建的缺省值必须落在
    /// `.invalid` 保留域——反目标「测试永不触生产端点」的编译期保障。
    /// 构建期注入了真实地址时本断言红:那是有意配置,非默认保障态。
    #[test]
    fn defaults_are_independent_and_placeholder_safe() {
        let _g = ENV_LOCK.lock().unwrap();
        assert_ne!(
            DEFAULT_SIGNALING_URL, DEFAULT_UPDATE_URL,
            "两个独立常量缺省值不应相同"
        );
        assert!(
            DEFAULT_SIGNALING_URL.ends_with(".invalid"),
            "未注入时信令缺省须为 .invalid 占位: {DEFAULT_SIGNALING_URL}"
        );
        assert!(
            DEFAULT_UPDATE_URL.ends_with(".invalid"),
            "未注入时更新缺省须为 .invalid 占位: {DEFAULT_UPDATE_URL}"
        );
    }
}
