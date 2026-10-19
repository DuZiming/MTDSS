//! 系统通知(接收完成提醒,M3c Task 5):notify-rust 的一发即忘封装。
//!
//! `notify_one` 是同步签名、异步语义:通知在专属线程里发出,调用方
//! (常驻接收循环)不因平台通知后端(WinRT toast / DBus)的阻塞或缺失
//! 受阻,也不在 reactor 线程上做阻塞系统调用。headless/无桌面会话/无
//! 总线(bus)环境下 notify-rust 失败仅 warn——通知是锦上添花,绝不
//! 反噬接收主链路;线程按发即弃(通知频率以接收轮计,每轮至多一条,
//! 线程创建成本可忽略)。

/// 发一条系统通知:专属线程内调用 notify-rust,失败仅 warn(见模块文档)。
pub(crate) fn notify_one(title: &str, body: &str) {
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
