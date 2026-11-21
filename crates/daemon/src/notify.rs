//! 系统通知(接收完成提醒,M3c Task 5):notify-rust 的一发即忘封装。
//!
//! `notify_one` 是同步签名、异步语义:通知在专属线程里发出,调用方
//! (常驻接收循环)不因平台通知后端(WinRT toast / DBus)的阻塞或缺失
//! 受阻,也不在 reactor 线程上做阻塞系统调用。headless/无桌面会话/无
//! 总线(bus)环境下 notify-rust 失败仅 warn——通知是锦上添花,绝不
//! 反噬接收主链路;线程按发即弃(通知频率以接收轮计,每轮至多一条,
//! 线程创建成本可忽略)。
//!
//! **测试构建门**(cfg(not(test)),先例 = lib 的 sweep_legacy_
//! autostart_registry 测试构建门):V4-1 T3 实测,全量 ctrl_e2e 负载下
//! notify-rust→windows-core 0.62 的 WinRT toast 在 'mtdss-notify' 线程上
//! 原生崩(misaligned pointer dereference → non-unwinding panic → abort,
//! 0xc0000409),整只测试二进制拖死且高度偶发(单测/小集不复现,全量
//! 负载两次全量跑两次崩)。测试进程本就不该弹系统 toast(桌面副作用不
//! 进测试进程),门掉即同时移除该崩溃族与弹窗面;生产面逐字节不变
//! (生产 daemon 单实例低频 toast,未见此崩形态——根因在 windows-core
//! 的 COM 封组化与测试进程内多 daemon 并发拓扑,列为票池候选)。

/// 发一条系统通知:专属线程内调用 notify-rust,失败仅 warn(见模块文档)。
#[cfg(not(test))]
pub(crate) fn notify_one(title: &str, body: &str) {
    // 测试进程门(集成测试链路):`cfg(test)` 只罩单测目标——集成测试
    // (tests/*.rs)链接的 lib 是**生产构建**,cfg 门罩不到;env 门由测试
    // 基建(`spawn_test_server`)在起任何 daemon 前预置,其后 spawn 的
    // mtdssd 子进程随环境继承同罩(V4-1 T3 实测教训,见模块注)。
    if test_notify_disabled() {
        tracing::debug!(title, "测试进程:系统通知门掉(MTDSS_TEST_DISABLE_NOTIFY)");
        return;
    }
    let title = title.to_string();
    let body = body.to_string();
    let spawned = std::thread::Builder::new()
        .name("mtdss-notify".into())
        .spawn(move || {
            // &str 形参:notify-rust 的 builder 同时兼容 &str/Into<String> 签名
            if let Err(e) = notify_rust::Notification::new()
                .summary(&title)
                .body(&body)
                .show()
            {
                tracing::warn!(%e, "系统通知发送失败(headless/无总线为预期路径)");
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(%e, "通知线程创建失败,本条通知丢弃");
    }
}

/// 测试进程判定(env 一次性读定;`MTDSS_TEST_DISABLE_NOTIFY=1` = 门)。
#[cfg_attr(test, allow(dead_code))] // 消费面 = cfg(not(test)) 生产体;测试构建置空体不用
fn test_notify_disabled() -> bool {
    static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| std::env::var("MTDSS_TEST_DISABLE_NOTIFY").ok().as_deref() == Some("1"))
}

/// 测试构建空体(见模块注「测试构建门」):调用面照常编译,不弹系统
/// toast、不起 notify 线程。
#[cfg(test)]
pub(crate) fn notify_one(_title: &str, _body: &str) {
    tracing::debug!("测试构建:系统通知门掉(notify-rust 全量负载崩,见模块注)");
}
