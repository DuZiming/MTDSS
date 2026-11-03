//! 进程内限流器(spec §3.10):滑动窗口计数,`DashMap<key, Vec<Instant>>`。
//! 单进程内存态,重启即清——本规模接受,不建表。`max`/`window` 由调用方
//! 按面给定(同一 Throttle 实例服务多个免认证端点与多档限额)。
//!
//! **键存储有界**(review Important #1):键可被任意未认证请求铸造(限速
//! 先于已知账号 404 门),破窗条目若不摘除将随请求量永驻=远程内存增长面。
//! 摊销全局清扫:每 [`SWEEP_EVERY`] 次 `allow` 一轮,摘除事件全旧于
//! [`EVENT_MAX_WINDOW`] 的条目——内存上界 = 活跃键 + 单清扫间隔内新铸键。

use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// 摊销清扫触发间隔(allow 调用数;request-code 三档=每请求 3 次,约
/// 1365 请求一轮)。
pub const SWEEP_EVERY: u64 = 4096;

/// 清扫的事件过期上界 = 免认证面最长限流窗口(per-email 5/h、per-IP 10/h
/// 均 1h,见 api.rs)。比它更旧的事件对任何在用检查都无影响,可安全摘除。
/// 不得用「当前调用的 window」清扫——短窗清扫会误摘 1h 桶历史,等于重置
/// 限流(攻击者获益)。
const EVENT_MAX_WINDOW: Duration = Duration::from_secs(3600);

pub struct Throttle {
    events: DashMap<String, Vec<Instant>>,
    /// allow 调用计数;每到 SWEEP_EVERY 触发一轮全局清扫
    calls: AtomicU64,
    /// 清扫 horizon(生产=EVENT_MAX_WINDOW;测试经 `_with_sweep_window_for_test` 注入短窗)
    sweep_window: Duration,
}

impl Default for Throttle {
    fn default() -> Self {
        Self {
            events: DashMap::new(),
            calls: AtomicU64::new(0),
            sweep_window: EVENT_MAX_WINDOW,
        }
    }
}

impl Throttle {
    pub fn new() -> Self {
        Self::default()
    }

    /// 滑动窗口判定:`window` 内记录不足 `max` 次 → 记一次并放行(true);
    /// 已满 → 拒绝(false),本次不记录。
    pub fn allow(&self, key: &str, max: u32, window: Duration) -> bool {
        let now = Instant::now();
        let mut v = self.events.entry(key.to_owned()).or_default();
        v.retain(|t| now.duration_since(*t) < window);
        let allowed = v.len() < max as usize;
        if allowed {
            v.push(now);
        }
        drop(v);
        // 摊销全局清扫:破窗键若不摘,免认证端点可刷爆键存储(review Important #1)
        let calls = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
        if calls.is_multiple_of(SWEEP_EVERY) {
            self.sweep(now, self.sweep_window);
        }
        allowed
    }

    /// 拒绝后距最早一条记录滑出窗口(腾出名额)还需等待的秒数(Retry-After
    /// 用;至少 1,拒绝时该键窗口必满,最早者先过期)。
    pub fn retry_after_secs(&self, key: &str, window: Duration) -> u64 {
        let now = Instant::now();
        match self.events.get(key) {
            Some(v) => v
                .iter()
                .filter_map(|t| {
                    let live = window.checked_sub(now.duration_since(*t))?;
                    Some(live.as_secs() + u64::from(live.subsec_nanos() > 0))
                })
                .min()
                .max(Some(1))
                .unwrap_or(1),
            None => 1,
        }
    }

    /// 全局清扫:摘除事件全旧于 `horizon` 的条目(`retain` 原地摘键,先清死事件)。
    fn sweep(&self, now: Instant, horizon: Duration) {
        self.events.retain(|_, v| {
            v.retain(|t| now.duration_since(*t) < horizon);
            !v.is_empty()
        });
    }

    /// 键数(观测/测试用)。
    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// 测试钩子:注入清扫 horizon(生产恒 [`EVENT_MAX_WINDOW`];
    /// `auth::_expire_all_codes_for_test` 同款先例)。
    pub fn _with_sweep_window_for_test(horizon: Duration) -> Self {
        Self {
            events: DashMap::new(),
            calls: AtomicU64::new(0),
            sweep_window: horizon,
        }
    }

    /// 测试钩子:按构造时注入的 horizon 立即清扫(生产清扫由 allow 摊销触发;
    /// 时间不可倒流,短窗 horizon 须注入才能测)。
    pub fn _sweep_for_test(&self) {
        self.sweep(Instant::now(), self.sweep_window);
    }
}
