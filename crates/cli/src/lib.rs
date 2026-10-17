pub mod flow;
// session 已迁入 mtdss-daemon(Task 3);re-export 保住 `crate::session::…`
// (flow.rs)与 `mtdss_cli::session::…`(tests)两条路径,行为不变。
// Task 5 迁移 flow 后,本 shim 随 cli Cargo.toml 的临时依赖一并撤销。
pub use mtdss_daemon::session;
