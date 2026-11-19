//! 预览句柄注册表与生命周期(V4-1 T4)。
//!
//! handle = 短时句柄(brief 接口面 `(device_id?,path,mtime?,size?)` 换
//! handle,mtime/size 变更即失效重申请,spec §2):preview_open 以退化键去
//! 重——mtime 在场 = 时变键 `(device,path,mtime)`;mtime 缺省 = **(path,
//! size) 退化键** `(device,path,FallbackSize(size))`(proto 注:请求方以
//! (path,size) 作预览缓存键;浏览条目恒带 size,size 双缺 = 0 兜底同键)。
//! 同键再开 = 引用计数 +1 返回**同一 handle**(面板+弹出窗多面共享一源,
//! spec §7「引用计数归零即停读」);preview_close 减计,归零即关闭(源句
//! 柄释放、在途读即停)。
//!
//! 生命周期三面:①引用计数(open ++ / close --);②**60s 空闲兜底回收**
//! (Review#2:UI 崩溃/漏 close 的句柄不挂死 fd——last_used 随每次 resolve
//! 刷新,惰性清扫 + 常驻 ticker 双保险);③单 handle 并发 range 请求上限
//! [`PER_HANDLE_RANGE_CAP`](spec §2:视频 seek 与 pdf 翻页互不打架)。
//!
//! 纪律:注册表只管**生命周期**(键/计数/空闲/并发帽),不管 HTTP 与缓存
//! (loopback.rs 面);源构造也归调用方(api.rs 装配本机/远端臂)——open
//! 收现成 `Arc<dyn FileSource>`,声明 size 核对(源快照不符 = 文件已变更
//! → [`OpenError::Stale`],对齐 read_range_reply 的 NOT_FOUND 口径)在此
//! 通用化,去重命中同样核对(entry.size 为开源时权威快照)。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::source::FileSource;

/// 空闲兜底回收界(Review#2):60s 无任何路由命中即回收(fd/计数面)。
pub(crate) const HANDLE_IDLE_TTL: Duration = Duration::from_secs(60);

/// 单 handle 并发 range 请求帽(spec §2;常量锚改值须过 handles 测试)。
pub(crate) const PER_HANDLE_RANGE_CAP: usize = 4;

/// handle 串字节数(16B 随机 = 32 hex;仅本机进程可达面,防碰撞即可)。
const HANDLE_BYTES: usize = 16;

/// 退化键第三分量(mtime 在场 = 时变键;缺省 = (path,size) 退化键)。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum KeyStamp {
    Mtime(i64),
    FallbackSize(u64),
}

/// 去重键:`(device, path, stamp)`。device 为解析后的本机/远端设备 id
/// (api.rs 装配:device_id 缺省 = self;未登录本地预览 = "local")。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct HandleKey {
    device: String,
    path: String,
    stamp: KeyStamp,
}

/// 打开失败:声明 size 与源不符(文件已变更,UI 重新浏览取新键)。源构造
/// 的 io 错误在调用方装配处即映业务错,注册表层只剩去重核对面。
#[derive(Debug)]
pub(crate) enum OpenError {
    Stale(String),
}

/// close 结果(单测锚;IPC 面一律幂等 `{}`,不外泄差异)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseOutcome {
    /// 计数归零,条目已关闭移除(在途读即停)。
    Removed,
    /// 计数尚有余,条目仍活。
    Released,
    /// 未知/已过期 handle(幂等面)。
    Unknown,
}

/// 注册表内条目:源 + 并发帽 + 生命周期旗标。HTTP 面持 `Arc<Entry>` 读。
pub(crate) struct Entry {
    /// 条目 id(单调分配,不复用;预读缓存键前缀——close 后旧缓存块不可达)。
    pub(crate) id: u64,
    /// 去重键(随条目存:close/回收清 by_key 无需反查)。
    key: HandleKey,
    /// 源字节数(开源时快照,authoritative;Range 数学与声明核对基准)。
    pub(crate) size: u64,
    pub(crate) source: Arc<dyn FileSource>,
    /// 并发 range 请求帽(第 5 路在信号量上排队,不拒)。
    pub(crate) range_permits: Arc<tokio::sync::Semaphore>,
    closed: AtomicBool,
    refs: AtomicU64,
    last_used: Mutex<std::time::Instant>,
}

impl Entry {
    /// 条目是否已关闭(close 归零/空闲回收后;在途读据此即停)。
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// 空闲时钟刷新(路由命中与 raw 逐块发放即 touch——活动流永不过界;
    /// V4-1 T4 评审 Important #1)。
    pub(crate) fn touch(&self) {
        *self.last_used.lock().unwrap() = std::time::Instant::now();
    }

    fn idle_for(&self) -> Duration {
        self.last_used.lock().unwrap().elapsed()
    }
}

/// 预览句柄注册表(std 锁不跨 await;open 的源构造在调用方,本表全同步)。
/// 随 state 常驻(进程内单份;测试态独立构造互不串扰)。
pub(crate) struct HandleRegistry {
    inner: Mutex<RegistryState>,
    /// 空闲兜底回收界(构造注入;生产 = [`HANDLE_IDLE_TTL`],测试注入小值)。
    idle_ttl: Duration,
    /// 常驻清扫任务启动旗标(首个 server 启动时置位,只 spawn 一次)。
    sweeper_started: AtomicBool,
}

#[derive(Default)]
struct RegistryState {
    by_handle: HashMap<String, Arc<Entry>>,
    by_key: HashMap<HandleKey, String>,
    seq: u64,
}

/// handle 串生成(16B 随机 → 32 hex;thread_rng 与 flow 会话 id 同款纪律)。
fn mint_handle() -> String {
    use rand::RngCore as _;
    let mut b = [0u8; HANDLE_BYTES];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

impl HandleRegistry {
    /// 生产构造(常量锚)。
    pub(crate) fn new() -> Self {
        Self::with_params(HANDLE_IDLE_TTL)
    }

    /// 参数注入构造(生产 `new` 与测试面共用;空闲界可调)。
    pub(crate) fn with_params(idle_ttl: Duration) -> Self {
        Self {
            inner: Mutex::new(RegistryState::default()),
            idle_ttl,
            sweeper_started: AtomicBool::new(false),
        }
    }

    /// 开句柄:同键在场 = 引用计数 +1 返回同一 handle(声明 size 与条目
    /// 快照核对);否则收下调用方装配的源建新条目。声明 size 在场且与源
    /// 快照不符 = [`OpenError::Stale`](文件已变更,UI 重新浏览)。
    pub(crate) fn open(
        &self,
        device: &str,
        path: &str,
        mtime: Option<i64>,
        declared: Option<u64>,
        source: Arc<dyn FileSource>,
    ) -> Result<String, OpenError> {
        // 声明核对先行(去重命中与新建两臂都过——条目 size 即开源快照的
        // 权威面;不符 = 文件已变更,照开旧键会污染缓存键语义)
        let size = source.size();
        if let Some(d) = declared {
            if d != size {
                return Err(OpenError::Stale(format!(
                    "文件已变更(声明 {d} ≠ 源 {size}),请重新浏览"
                )));
            }
        }
        let stamp = match mtime {
            Some(m) => KeyStamp::Mtime(m),
            // 退化键 (path,size);size 双缺 = 0 兜底同键(proto 注口径)
            None => KeyStamp::FallbackSize(declared.unwrap_or(0)),
        };
        let key = HandleKey {
            device: device.to_string(),
            path: path.to_string(),
            stamp,
        };
        let mut st = self.inner.lock().unwrap();
        if let Some(h) = st.by_key.get(&key).cloned() {
            if let Some(e) = st.by_handle.get(&h) {
                e.refs.fetch_add(1, Ordering::SeqCst);
                e.touch();
                return Ok(h);
            }
            // by_key 残影(by_handle 已清)= 不可能态,清残影走新建
            st.by_key.remove(&key);
        }
        let id = st.seq;
        st.seq += 1;
        let handle = mint_handle();
        let entry = Arc::new(Entry {
            id,
            key: key.clone(),
            size,
            source,
            range_permits: Arc::new(tokio::sync::Semaphore::new(PER_HANDLE_RANGE_CAP)),
            closed: AtomicBool::new(false),
            refs: AtomicU64::new(1),
            last_used: Mutex::new(std::time::Instant::now()),
        });
        st.by_key.insert(key, handle.clone());
        st.by_handle.insert(handle.clone(), entry);
        Ok(handle)
    }

    /// 关句柄(引用计数 -1):归零 = 关闭移除(`Removed`);尚有余 =
    /// `Released`;未知/已过期 = `Unknown`(幂等面)。
    pub(crate) fn close(&self, handle: &str) -> CloseOutcome {
        let mut st = self.inner.lock().unwrap();
        let Some(entry) = st.by_handle.get(handle).cloned() else {
            return CloseOutcome::Unknown;
        };
        // 归零收口:先摘表再落旗标——旗标是最后一步,保证「在表内条目必
        // 未 closed」的观察序(resolve 永不吐 closed 条目)
        let prev = entry.refs.fetch_sub(1, Ordering::SeqCst);
        if prev > 1 {
            entry.touch();
            return CloseOutcome::Released;
        }
        st.by_handle.remove(handle);
        st.by_key.remove(&entry.key);
        entry.closed.store(true, Ordering::SeqCst);
        CloseOutcome::Removed
    }

    /// 路由面解析:活条目即刷新空闲时钟并返回;关闭/过期/未知 = None
    /// (HTTP 面 410)。顺带惰性清扫(常驻 ticker 的兜底补充)。
    pub(crate) fn resolve(&self, handle: &str) -> Option<Arc<Entry>> {
        self.sweep();
        let st = self.inner.lock().unwrap();
        let e = st.by_handle.get(handle).cloned()?;
        e.touch();
        Some(e)
    }

    /// 空闲清扫:超 [`Self::idle_ttl`] 未命中的条目一律关闭移除(UI 崩溃
    /// 漏 close 的兜底;在途读即停)。**在途豁免**(评审 Important #1):
    /// 持有 range permit 的条目一律不回收——permit 从请求入帽持到流体终
    /// 了,是「请求在途」的权威面;慢源单块装货(远端 = 一次 ctrl 往返)
    /// 可以任意久不产块,只靠逐块 touch 会在块间隙漏判,持帽即豁免才闭合
    /// 「活动流被空闲回收拦腰截断,与短读不可区分」的洞。
    pub(crate) fn sweep(&self) {
        let mut st = self.inner.lock().unwrap();
        let ttl = self.idle_ttl;
        let dead: Vec<String> = st
            .by_handle
            .iter()
            .filter(|(_, e)| {
                e.idle_for() > ttl && e.range_permits.available_permits() == PER_HANDLE_RANGE_CAP
            })
            .map(|(h, _)| h.clone())
            .collect();
        for h in dead {
            if let Some(e) = st.by_handle.remove(&h) {
                st.by_key.remove(&e.key);
                e.closed.store(true, Ordering::SeqCst);
            }
        }
    }

    /// 常驻清扫任务(首个 server 启动时拉起;间隔 = ttl/3 下限 500ms——
    /// 兜底回收至迟 ttl+一个间隔内生效)。重复调用幂等(AtomicBool 门,
    /// 复调回一个即毕任务的占位句柄)。**句柄归调用方**(V4-1 T4 评审
    /// Important #2:server 持有并在 daemon 收口时 abort——清扫任务不得
    /// 出活得比 daemon 久的泄漏任务)。清扫循环体 = interval + sweep,
    /// 无 I/O 半程态,abort 即净收口。
    pub(crate) fn spawn_sweeper(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        if self.sweeper_started.swap(true, Ordering::SeqCst) {
            return tokio::spawn(async {});
        }
        let reg = Arc::clone(self);
        let interval = (self.idle_ttl / 3).max(Duration::from_millis(500));
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            loop {
                tick.tick().await;
                reg.sweep();
            }
        })
    }

    /// 活条目数(单测锚)。
    #[cfg(test)]
    fn live_count(&self) -> usize {
        self.inner.lock().unwrap().by_handle.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    /// 内存假源(单测面):固定 size,读出零;可选读失败面。
    struct MockSource {
        size: u64,
        fail: bool,
    }
    impl MockSource {
        fn ok(size: u64) -> Arc<dyn FileSource> {
            Arc::new(MockSource { size, fail: false })
        }
    }
    impl FileSource for MockSource {
        fn size(&self) -> u64 {
            self.size
        }
        fn read_at(&self, _off: u64, buf: &mut [u8]) -> io::Result<usize> {
            if self.fail {
                return Err(io::Error::other("mock 读失败"));
            }
            Ok(buf.len())
        }
    }

    fn reg() -> HandleRegistry {
        HandleRegistry::with_params(Duration::from_secs(60))
    }

    /// 同键去重 + 引用计数:mtime 同值两开 = 同 handle;close 一枚仍活,
    /// 两枚全关即 Removed 且 resolve None。mtime/size 双缺 = FallbackSize(0)
    /// 同键(退化键面)。
    #[test]
    fn open_dedups_by_key_and_refcounts() {
        let r = reg();
        let src = MockSource::ok(100);
        let h1 = r.open("d", "/p", Some(5), Some(100), src.clone()).unwrap();
        let h2 = r
            .open("d", "/p", Some(5), Some(100), MockSource::ok(100))
            .unwrap();
        assert_eq!(h1, h2, "同键两开返回同一 handle");
        assert_eq!(r.live_count(), 1, "单条目");
        assert_eq!(r.close(&h1), CloseOutcome::Released, "计数尚余");
        assert!(r.resolve(&h1).is_some(), "仍有引用,条目活");

        let h3 = r
            .open("d", "/p", Some(5), Some(100), MockSource::ok(100))
            .unwrap();
        assert_eq!(h3, h1, "关一开一仍回同键条目");
        assert_eq!(r.close(&h2), CloseOutcome::Released);
        assert_eq!(r.close(&h3), CloseOutcome::Removed, "计数归零即关闭");
        assert!(r.resolve(&h1).is_none(), "关闭后 resolve None(HTTP 410)");
        assert_eq!(r.close(&h1), CloseOutcome::Unknown, "再关 = 幂等 Unknown");
    }

    /// 退化键(mtime 缺省):size 同 → 同 handle;size 异 → 异 handle;
    /// mtime 在场时 size 不入键(mtime 同 size 异仍同键);mtime 异 → 异键。
    #[test]
    fn fallback_key_uses_size_when_mtime_absent() {
        let r = reg();
        let a = r
            .open("d", "/p", None, Some(100), MockSource::ok(100))
            .unwrap();
        let b = r
            .open("d", "/p", None, Some(100), MockSource::ok(100))
            .unwrap();
        assert_eq!(a, b, "mtime 缺省同 (path,size) = 同键");
        let c = r
            .open("d", "/p", None, Some(200), MockSource::ok(200))
            .unwrap();
        assert_ne!(a, c, "size 异 = 异键(退化键面)");
        let d = r
            .open("d", "/p", Some(7), Some(100), MockSource::ok(100))
            .unwrap();
        assert_ne!(a, d, "mtime 在场 = 时变键,与退化键互异");
        // mtime 同则 size 不入键:声明缺席(不核对)即证——同 mtime 再开
        // 回同一条目;若 size 入键,这一发会造出新 handle。
        let e = r
            .open("d", "/p", Some(7), None, MockSource::ok(100))
            .unwrap();
        assert_eq!(d, e, "mtime 同则 size 不入键(声明缺席不核对)");
        let f = r
            .open("d", "/p", Some(8), Some(100), MockSource::ok(100))
            .unwrap();
        assert_ne!(d, f, "mtime 异 = 异键(mtime 变更即失效重申请)");
        // 双缺(mtime、size 皆无)= FallbackSize(0) 兜底同键
        let g1 = r.open("d", "/q", None, None, MockSource::ok(1)).unwrap();
        let g2 = r.open("d", "/q", None, None, MockSource::ok(1)).unwrap();
        assert_eq!(g1, g2, "双缺 = FallbackSize(0) 同键");
    }

    /// 声明 size 核对:在场且与源不符 = Stale(新开与去重命中两臂都核对)。
    #[test]
    fn declared_size_mismatch_is_stale() {
        let r = reg();
        let got = r.open("d", "/p", Some(5), Some(99), MockSource::ok(100));
        assert!(
            matches!(got, Err(OpenError::Stale(_))),
            "新开臂:声明不符 = Stale: {got:?}"
        );
        let h = r
            .open("d", "/p", Some(5), Some(100), MockSource::ok(100))
            .unwrap();
        let got = r.open("d", "/p", Some(5), Some(99), MockSource::ok(100));
        assert!(
            matches!(got, Err(OpenError::Stale(_))),
            "去重命中臂:声明与条目快照不符 = Stale"
        );
        assert!(r.resolve(&h).is_some(), "Stale 核对不损既有条目");
        let got = r.open("d", "/p", Some(5), None, MockSource::ok(100));
        assert!(got.is_ok(), "声明缺席 = 不核对");
    }

    /// 空闲兜底回收:超 ttl 未命中即回收;命中刷新时钟(ttl 内两命中间隔
    /// 不超界 = 存活跨两个 ttl);sweep 直驱同面。窗值留调度余量(并行负载
    /// 下毫秒级抖动不翻窗——「未超界存活」的断言窗必须宽过抖动上界)。
    #[test]
    fn idle_ttl_reclaims_and_use_refreshes() {
        let ttl = Duration::from_millis(240);
        let r = HandleRegistry::with_params(ttl);
        let h = r
            .open("d", "/p", Some(1), Some(10), MockSource::ok(10))
            .unwrap();
        std::thread::sleep(ttl * 2);
        assert!(r.resolve(&h).is_none(), "超界未命中 = 空闲回收(HTTP 410)");
        assert_eq!(r.live_count(), 0, "回收后条目移除");

        let h = r
            .open("d", "/p", Some(1), Some(10), MockSource::ok(10))
            .unwrap();
        std::thread::sleep(ttl / 2);
        assert!(r.resolve(&h).is_some(), "界内命中存活");
        std::thread::sleep(ttl / 2);
        assert!(r.resolve(&h).is_some(), "命中刷新时钟,跨第二窗仍活");
        std::thread::sleep(ttl * 2);
        assert!(r.resolve(&h).is_none(), "停用后超界回收");

        // sweep 直驱面(ticker 的同步本体)
        let h = r
            .open("d", "/p", Some(1), Some(10), MockSource::ok(10))
            .unwrap();
        std::thread::sleep(ttl * 2);
        r.sweep();
        assert_eq!(r.live_count(), 0, "sweep 直驱回收");
        assert_eq!(r.close(&h), CloseOutcome::Unknown, "已回收 = Unknown");
    }

    /// 单 handle 并发 range 帽 = 4:四枚即取即得,第五枚排队(短窗超时
    /// 证未得);放一枚即得。帽常量锚(改值须过本断言)。
    #[tokio::test]
    async fn per_handle_range_cap_four() {
        assert_eq!(PER_HANDLE_RANGE_CAP, 4, "并发帽常量锚");
        let r = reg();
        let h = r
            .open("d", "/p", Some(1), Some(10), MockSource::ok(10))
            .unwrap();
        let e = r.resolve(&h).expect("条目在场");
        let mut held = Vec::new();
        for _ in 0..PER_HANDLE_RANGE_CAP {
            held.push(
                e.range_permits
                    .clone()
                    .acquire_owned()
                    .await
                    .expect("帽内即得"),
            );
        }
        let mut fifth = tokio::spawn({
            let p = e.range_permits.clone();
            async move { p.acquire_owned().await.expect("信号量不关") }
        });
        let overflow = tokio::time::timeout(Duration::from_millis(150), &mut fifth).await;
        assert!(overflow.is_err(), "第五枚必须排队(帽满): {overflow:?}");
        drop(held.pop());
        let got_fifth = tokio::time::timeout(Duration::from_secs(1), fifth)
            .await
            .expect("放一枚后第五枚即得")
            .expect("spawn 不崩");
        drop(got_fifth);
    }
}
