//! 预览独立限流(V4-1 T3):令牌桶三档优先级,浏览 > 预览 > 批量传输。
//!
//! 独立令牌桶 = 与传输引擎任何并发面(上传槽/接收轮/xfer 锁)完全无关:
//! 大文件批量传输进行中,预览按需读不占批量并发、也不被批量饿死——两端
//! 共享的只有磁盘,应用面并发由本桶单独圈定(e2e `preview_read_range_
//! segments_and_bulk_independence` 钉「批量进行中预览首字节 <2s」)。
//!
//! 折中纪律(spec「不饿死也不被饿死」):高档就绪即优先得票;低档等待
//! 老化晋升界 [`PREVIEW_WAIT_AGING`] 后按最高档参与分配——浏览洪水不至于
//! 永久压死预览,批量洪峰也不会无限迟滞浏览。参数全是常量并注释可调。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// 消费档位(优先级升序声明:枚举值即档位秩,高值优先)。优先级语义:
/// **浏览 > 预览 > 批量传输**——浏览(list/stat 级交互)最优先,预览次之,
/// 批量传输垫底。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PreviewClass {
    /// 批量传输(最低档;T8/T10 手机桥导出批量读挂入)
    #[cfg_attr(not(test), allow(dead_code))] // T3 消费面仅 Preview;T8/T10 挂入 Bulk
    Bulk = 0,
    /// 预览(中档;T3 file_read_range 读源消费)
    Preview = 1,
    /// 浏览(最高档;T7/T12 转码/缩略图会话首开段可挂入)
    #[cfg_attr(not(test), allow(dead_code))] // T3 消费面仅 Preview;T7/T12 挂入 Browse
    Browse = 2,
}

/// 并发读源令牌数(常量锚;改值须过本常量的引用面)。2 = 一路预览流 + 一
/// 路排队余量:预览段读是秒级小窗,令牌随生产任务终了即回,排队等待通常
/// 亚百毫秒(可调:慢盘实机调大,弱机调小)。
pub(crate) const PREVIEW_SOURCE_TOKENS: usize = 2;

/// 等待老化晋升界:低档等待超过该时长,参与分配时按最高档
/// ([`PreviewClass::Browse`])计——「不饿死也不被饿死」的折中(可调:值
/// 小=更快扶低档,值大=高档优先更彻底)。
pub(crate) const PREVIEW_WAIT_AGING: Duration = Duration::from_secs(10);

/// 老化晋升纯函数(单测钉三臂):等待超界即按最高档参与抢占。
fn effective_class(class: PreviewClass, waited: Duration, aging: Duration) -> PreviewClass {
    if waited >= aging {
        PreviewClass::Browse
    } else {
        class
    }
}

/// 等待者(入队时登记档位与时刻;得票经 oneshot 直送)。
struct Waiter {
    class: PreviewClass,
    enqueued: Instant,
    tx: oneshot::Sender<()>,
}

#[derive(Default)]
struct LimiterState {
    /// 空闲令牌(无等待者时令票在此驻留;有等待者时令票直接转交,free 不动)。
    free: usize,
    /// 等待队列(Vec push 序 = FIFO;分配时按老化晋升后的档位降序、同档
    /// 按入队序取最优)。
    waiters: Vec<Waiter>,
}

/// 预览独立限流器(std 锁不跨 await:enqueue/release 均即返,等待面在
/// oneshot 上)。随 DaemonState 常驻(进程内单份;测试态独立构造互不串扰)。
pub(crate) struct PreviewLimiter {
    inner: Mutex<LimiterState>,
    /// 老化晋升界(构造注入;生产 = [`PREVIEW_WAIT_AGING`],测试注入小值)。
    aging: Duration,
}

/// 令牌手柄:持有一个读源令牌;drop 即归还(有等待者直送,否则入空闲)。
pub(crate) struct PreviewPermit {
    limiter: Option<Arc<PreviewLimiter>>,
}

impl PreviewLimiter {
    /// 生产构造(常量锚)。
    pub(crate) fn new() -> Self {
        Self::with_params(PREVIEW_SOURCE_TOKENS, PREVIEW_WAIT_AGING)
    }

    /// 参数注入构造(测试面:令牌数与老化界各自可调)。
    pub(crate) fn with_params(tokens: usize, aging: Duration) -> Self {
        Self {
            inner: Mutex::new(LimiterState {
                free: tokens,
                waiters: Vec::new(),
            }),
            aging,
        }
    }

    /// 即取令牌:空闲票在场即得,否则 None(不等待)。单测与探测面消费
    /// (T3 生产面走 [`Self::acquire`] 等待臂)。
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn try_acquire(self: &Arc<Self>, _class: PreviewClass) -> Option<PreviewPermit> {
        let mut st = self.inner.lock().unwrap();
        if st.free == 0 {
            return None;
        }
        st.free -= 1;
        Some(PreviewPermit {
            limiter: Some(Arc::clone(self)),
        })
    }
    /// 取令牌(等待面):空闲票即得;否则入队,等待高档让票或老化晋升。
    /// 档位在**入队**时登记,分配时按当时的老化效果参与。`self: Arc<Self>`
    /// 按值收口:future 持自有句柄,可安全 tokio::spawn(排队面并发测试用)。
    pub(crate) async fn acquire(self: Arc<Self>, class: PreviewClass) -> PreviewPermit {
        let rx = {
            let mut st = self.inner.lock().unwrap();
            if st.free > 0 {
                st.free -= 1;
                None
            } else {
                let (tx, rx) = oneshot::channel();
                st.waiters.push(Waiter {
                    class,
                    enqueued: Instant::now(),
                    tx,
                });
                Some(rx)
            }
        };
        if let Some(rx) = rx {
            // 等直送票(permit drop 时 release 选优直送);Err = 发票端缺席
            // (limiter 随 state 终结的测试边缘)——按已得票收口,归还是
            // 对已终结 limiter 的无害尾写。
            let _ = rx.await;
        }
        PreviewPermit {
            limiter: Some(self),
        }
    }

    /// 归还一枚令牌:有等待者 = 老化晋升后的档位降序、同档 FIFO 取最优
    /// 直送(free 不动);无等待者 = 入空闲。
    fn release(&self) {
        let mut st = self.inner.lock().unwrap();
        let best = st
            .waiters
            .iter()
            .enumerate()
            .max_by_key(|(i, w)| {
                (
                    effective_class(w.class, w.enqueued.elapsed(), self.aging) as u8,
                    std::cmp::Reverse(*i),
                )
            })
            .map(|(i, _)| i);
        match best {
            Some(i) => {
                let w = st.waiters.remove(i);
                let _ = w.tx.send(());
            }
            None => st.free += 1,
        }
    }

    /// 在队等待者数(单测确定性锚:spawn 的等待者**真实入队**后才推进
    /// 用例步,免 yield_now 调度序的脆墙钟——排序断言必须是确定的)。
    #[cfg(test)]
    fn pending_waiters(&self) -> usize {
        self.inner.lock().unwrap().waiters.len()
    }
}

impl Drop for PreviewPermit {
    fn drop(&mut self) {
        if let Some(limiter) = self.limiter.take() {
            limiter.release();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn limiter(tokens: usize, aging: Duration) -> Arc<PreviewLimiter> {
        Arc::new(PreviewLimiter::with_params(tokens, aging))
    }

    /// 老化晋升纯函数三臂:未超界保档;超界(含恰好等于)按 Browse。
    #[test]
    fn aging_effective_class_pins_arms() {
        assert_eq!(
            effective_class(
                PreviewClass::Bulk,
                Duration::from_secs(1),
                PREVIEW_WAIT_AGING
            ),
            PreviewClass::Bulk,
            "未超界保档"
        );
        assert_eq!(
            effective_class(PreviewClass::Bulk, PREVIEW_WAIT_AGING, PREVIEW_WAIT_AGING),
            PreviewClass::Browse,
            "恰达界即晋升"
        );
        assert_eq!(
            effective_class(
                PreviewClass::Preview,
                PREVIEW_WAIT_AGING * 2,
                PREVIEW_WAIT_AGING
            ),
            PreviewClass::Browse,
            "超界晋升最高档"
        );
    }

    /// 档位秩声明即优先级(常量锚):Browse > Preview > Bulk。
    #[test]
    fn class_rank_pins_priority_order() {
        assert!(PreviewClass::Browse > PreviewClass::Preview);
        assert!(PreviewClass::Preview > PreviewClass::Bulk);
        assert_eq!(PreviewClass::Browse as u8, 2);
        assert_eq!(PREVIEW_SOURCE_TOKENS, 2, "令牌数常量锚(改值须过本断言)");
    }

    /// 令牌帽与归还:满帽 try_acquire None;归一还一;release 无等待者入
    /// 空闲,try_acquire 复得。
    #[test]
    fn cap_and_release_recycle() {
        let l = limiter(2, PREVIEW_WAIT_AGING);
        let p1 = l.try_acquire(PreviewClass::Preview).expect("票一");
        let p2 = l.try_acquire(PreviewClass::Bulk).expect("票二");
        assert!(l.try_acquire(PreviewClass::Browse).is_none(), "满帽即拒");
        drop(p1);
        assert!(l.try_acquire(PreviewClass::Browse).is_some(), "归还即复得");
        drop(p2);
        assert!(l.try_acquire(PreviewClass::Bulk).is_some(), "第二枚复得");
    }

    /// 优先级分配:满帽下按 Bulk→Preview→Browse 顺序排队(以 pending_
    /// waiters 锚定「真实入队」),逐枚归还时按 Browse→Preview→Bulk 得票
    /// (高档先得;同档 FIFO——Bulk 先入队,最后得票)。
    #[tokio::test]
    async fn priority_order_browse_first() {
        let l = limiter(1, PREVIEW_WAIT_AGING);
        let _held = l.clone().acquire(PreviewClass::Bulk).await; // 占满
                                                                 // 排队:Bulk 先入队(同档 FIFO 对照),再 Preview,再 Browse
        let bulk = tokio::spawn(l.clone().acquire(PreviewClass::Bulk));
        while l.pending_waiters() < 1 {
            tokio::task::yield_now().await;
        }
        let preview = tokio::spawn(l.clone().acquire(PreviewClass::Preview));
        while l.pending_waiters() < 2 {
            tokio::task::yield_now().await;
        }
        let browse = tokio::spawn(l.clone().acquire(PreviewClass::Browse));
        while l.pending_waiters() < 3 {
            tokio::task::yield_now().await;
        }
        // 逐枚归还:Browse 先得,再 Preview,Bulk 最后
        drop(_held);
        let got_browse = tokio::time::timeout(Duration::from_secs(1), browse)
            .await
            .expect("Browse 不得超时")
            .expect("spawn 不崩");
        drop(got_browse);
        let got_preview = tokio::time::timeout(Duration::from_secs(1), preview)
            .await
            .expect("Preview 不得超时")
            .expect("spawn 不崩");
        drop(got_preview);
        let got_bulk = tokio::time::timeout(Duration::from_secs(1), bulk)
            .await
            .expect("Bulk 不得超时")
            .expect("spawn 不崩");
        drop(got_bulk);
        assert!(
            l.try_acquire(PreviewClass::Preview).is_some(),
            "全部归还后票回空闲"
        );
    }

    /// 老化晋升:注入 50ms 老化界,Bulk 等待老化超界后,新到队的 Preview
    /// 不得越过它——下一枚归还票按老化后的最高档直送 Bulk。
    #[tokio::test]
    async fn aging_promotes_starved_waiter_over_fresh_high_class() {
        let l = limiter(1, Duration::from_millis(50));
        let _held = l.clone().acquire(PreviewClass::Bulk).await;
        let aged_bulk = tokio::spawn(l.clone().acquire(PreviewClass::Bulk));
        while l.pending_waiters() < 1 {
            tokio::task::yield_now().await;
        }
        // 等 Bulk 老化超界后,再入队一个新 Preview(未老化,档位低于晋升后的 Bulk)
        tokio::time::sleep(Duration::from_millis(150)).await;
        let fresh_preview = tokio::spawn(l.clone().acquire(PreviewClass::Preview));
        while l.pending_waiters() < 2 {
            tokio::task::yield_now().await;
        }
        drop(_held);
        let got_bulk = tokio::time::timeout(Duration::from_secs(1), aged_bulk)
            .await
            .expect("老化的 Bulk 必须先得票")
            .expect("spawn 不崩");
        drop(got_bulk);
        let got_preview = tokio::time::timeout(Duration::from_secs(1), fresh_preview)
            .await
            .expect("Fresh Preview 随后得票")
            .expect("spawn 不崩");
        drop(got_preview);
    }
}
