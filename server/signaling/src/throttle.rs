//! 进程内限流器(spec §3.10):滑动窗口计数,`DashMap<key, Vec<Instant>>`。
//! 单进程内存态,重启即清——本规模接受,不建表。`max`/`window` 由调用方
//! 按面给定(同一 Throttle 实例服务多个免认证端点与多档限额)。

use dashmap::DashMap;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Throttle {
    events: DashMap<String, Vec<Instant>>,
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
        if v.len() < max as usize {
            v.push(now);
            true
        } else {
            false
        }
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
}
