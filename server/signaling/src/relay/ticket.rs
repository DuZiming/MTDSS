//! 中继票据与纯逻辑:HMAC 票据/标签、会话注册表、令牌桶限速。
//! 本层无 I/O、不读时钟——时间一律由调用方注入 [`std::time::Instant`],
//! 供 UDP 数据面(T3)与 WS 分配面(T4)复用。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;

/// 会话空闲上限:距最近刷新超过该时长即被 [`Registry::sweep_idle`] 移除。
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// 中继会话票据:会话号 + 过期时刻(Unix 秒)。
#[derive(Debug, Clone, Copy)]
pub struct Ticket {
    pub sid: u64,
    pub expires_at: i64,
}

/// 标签派生:HMAC-SHA256(secret, b"mtdss-relay-tag" || sid.to_le_bytes()
/// || expires_at.to_le_bytes() || slot_byte)[..16]。
/// slot∈{0,1} 一票两签——数据面按 tag 识别发送方,NAT 换址不产生歧义。
fn derive_tag(secret: &[u8; 32], sid: u64, expires_at: i64, slot: u8) -> [u8; 16] {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC 接受任意长度密钥");
    mac.update(b"mtdss-relay-tag");
    mac.update(&sid.to_le_bytes());
    mac.update(&expires_at.to_le_bytes());
    mac.update(&[slot]);
    let out = mac.finalize().into_bytes();
    out[..16].try_into().expect("32 字节摘要截取前 16 字节")
}

/// 铸票:RNG 出随机会话号,再派生双向标签。
/// 同 (secret, sid, expires_at) 必得同标签对;注入 RNG 使确定性可被单测复现。
pub fn mint_ticket(
    secret: &[u8; 32],
    expires_at: i64,
    rng: &mut impl RngCore,
) -> (Ticket, [u8; 16], [u8; 16]) {
    let ticket = Ticket { sid: rng.next_u64(), expires_at };
    let tag_a = derive_tag(secret, ticket.sid, expires_at, 0);
    let tag_b = derive_tag(secret, ticket.sid, expires_at, 1);
    (ticket, tag_a, tag_b)
}

/// 中继会话:票据 + 归属/限速元数据 + 双向槽位地址 + 双桶。
pub struct Session {
    pub ticket: Ticket,
    pub account_id: String,
    pub rate_kbps: u32,
    /// 两个方向的最近对端地址(槽位 0/1),握手前为 None
    pub slot_addrs: [Option<SocketAddr>; 2],
    pub bucket_a: TokenBucket,
    pub bucket_b: TokenBucket,
    /// 会话累计已转发字节(配额落库由调用方负责,此处只累加)
    pub bytes_accounted: u64,
    /// 最近一次活动/保活刷新时刻(注入时钟;sweep 依据)
    pub last_refresh: Instant,
}

/// 令牌桶限速(kbps):按注入时钟惰性补币,满桶容量 = 1 秒配额;
/// 余额不足即拒绝且不动余额(丢弃式,不排队)。
pub struct TokenBucket {
    rate_kbps: u32,
    tokens: f64,
    last_refill: Option<Instant>,
}

impl TokenBucket {
    /// 满桶起步(容量 = 1 秒配额,允许小幅突发)。
    pub fn new(rate_kbps: u32) -> Self {
        let capacity = Self::capacity_of(rate_kbps);
        Self { rate_kbps, tokens: capacity, last_refill: None }
    }

    /// 一秒配额的字节数(kbps → B/s)。
    fn capacity_of(rate_kbps: u32) -> f64 {
        f64::from(rate_kbps) * 1000.0 / 8.0
    }

    /// 结清上次补币以来的增量并锚定到 now;首次调用只锚定(桶已满)。
    fn refill(&mut self, now: Instant) {
        if let Some(last) = self.last_refill {
            let cap = Self::capacity_of(self.rate_kbps);
            let accrued = now.saturating_duration_since(last).as_secs_f64() * cap;
            self.tokens = (self.tokens + accrued).min(cap);
        }
        self.last_refill = Some(now);
    }

    /// 惰性补币后尝试取走 bytes;余额足够则扣减并返回 true,不足返回 false。
    pub fn try_take(&mut self, bytes: usize, now: Instant) -> bool {
        self.refill(now);
        let want = bytes as f64;
        if self.tokens >= want {
            self.tokens -= want;
            true
        } else {
            false
        }
    }

    /// 热调速率:先按旧速率结清补币,再切容量;余额超出新容量即截断。
    pub fn set_rate(&mut self, rate_kbps: u32, now: Instant) {
        self.refill(now);
        self.rate_kbps = rate_kbps;
        self.tokens = self.tokens.min(Self::capacity_of(rate_kbps));
    }
}

/// 会话条目:连同一对槽签一起存,remove/sweep 时 O(1) 摘除双 tag。
struct SessionEntry {
    tag_a: [u8; 16],
    tag_b: [u8; 16],
    session: Session,
}

/// 会话注册表:tag → (sid, slot) 与 sid → Session 双索引。
/// lookup 只认标签(伪造/错票据/过期票签均查不到);过期由调用方凭
/// `Ticket::expires_at` 复核。
pub struct Registry {
    by_tag: HashMap<[u8; 16], (u64, u8)>,
    sessions: HashMap<u64, SessionEntry>,
}

impl Registry {
    pub fn new() -> Self {
        Self { by_tag: HashMap::new(), sessions: HashMap::new() }
    }

    /// 收录会话并登记双向标签。
    pub fn insert(&mut self, s: Session, tag_a: [u8; 16], tag_b: [u8; 16]) {
        let sid = s.ticket.sid;
        self.by_tag.insert(tag_a, (sid, 0));
        self.by_tag.insert(tag_b, (sid, 1));
        self.sessions.insert(sid, SessionEntry { tag_a, tag_b, session: s });
    }

    /// 按标签取 (sid, slot);非 16 字节或未登记一律 None,畸形输入不 panic。
    pub fn lookup(&self, tag: &[u8]) -> Option<(u64, u8)> {
        let key: [u8; 16] = tag.try_into().ok()?;
        self.by_tag.get(&key).copied()
    }

    pub fn get_mut(&mut self, sid: u64) -> Option<&mut Session> {
        self.sessions.get_mut(&sid).map(|e| &mut e.session)
    }

    /// 摘除会话并注销双标签;返回被摘会话。
    pub fn remove(&mut self, sid: u64) -> Option<Session> {
        let entry = self.sessions.remove(&sid)?;
        self.by_tag.remove(&entry.tag_a);
        self.by_tag.remove(&entry.tag_b);
        Some(entry.session)
    }

    /// 移除 `now - last_refresh > 120s` 的会话(阈值严格大于),返回被移除 sid。
    pub fn sweep_idle(&mut self, now: Instant) -> Vec<u64> {
        let stale: Vec<u64> = self
            .sessions
            .iter()
            .filter(|(_, e)| {
                now.saturating_duration_since(e.session.last_refresh) > IDLE_TIMEOUT
            })
            .map(|(sid, _)| *sid)
            .collect();
        for sid in &stale {
            self.remove(*sid);
        }
        stale
    }

    /// 该账号名下存活会话数。
    pub fn count_for_account(&self, account_id: &str) -> usize {
        self.sessions
            .values()
            .filter(|e| e.session.account_id == account_id)
            .count()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    const SECRET: [u8; 32] = [7u8; 32];

    /// 固定种子铸票:同种子 ⇒ 同 sid ⇒ 同标签对,确定性可复现。
    fn mint_with(seed: u64, expires_at: i64) -> (Ticket, [u8; 16], [u8; 16]) {
        mint_ticket(&SECRET, expires_at, &mut StdRng::seed_from_u64(seed))
    }

    fn session(sid: u64, account: &str, rate_kbps: u32, last_refresh: Instant) -> Session {
        Session {
            ticket: Ticket { sid, expires_at: 0 },
            account_id: account.to_string(),
            rate_kbps,
            slot_addrs: [None, None],
            bucket_a: TokenBucket::new(rate_kbps),
            bucket_b: TokenBucket::new(rate_kbps),
            bytes_accounted: 0,
            last_refresh,
        }
    }

    fn registry_with_one(seed: u64, expires_at: i64) -> (Registry, Ticket, [u8; 16], [u8; 16]) {
        let t0 = Instant::now();
        let (ticket, tag_a, tag_b) = mint_with(seed, expires_at);
        let mut reg = Registry::new();
        reg.insert(session(ticket.sid, "acc", 1000, t0), tag_a, tag_b);
        (reg, ticket, tag_a, tag_b)
    }

    #[test]
    fn tag_pair_is_deterministic_per_ticket() {
        let (t1, a1, b1) = mint_with(1, 1_000);
        let (t2, a2, b2) = mint_with(1, 1_000);
        assert_eq!(t1.sid, t2.sid);
        assert_eq!((a1, b1), (a2, b2));
        // slot_byte 参与派生:同票两向标签必不同(错 slot 的签认不了)
        assert_ne!(a1, b1);
        // 不同种子 ⇒ 不同会话 ⇒ 标签必不同
        let (t3, a3, b3) = mint_with(2, 1_000);
        assert_ne!(t1.sid, t3.sid);
        assert_ne!(a1, a3);
        assert_ne!(b1, b3);
    }

    #[test]
    fn registry_lookup_maps_tag_to_sid_and_slot() {
        let (reg, ticket, tag_a, tag_b) = registry_with_one(1, 2_000);
        assert_eq!(reg.lookup(&tag_a), Some((ticket.sid, 0)));
        assert_eq!(reg.lookup(&tag_b), Some((ticket.sid, 1)));
    }

    #[test]
    fn registry_rejects_forged_wrong_ticket_and_expired_tags() {
        let (reg, _, tag_a, _) = registry_with_one(1, 2_000);
        // 错票据:另一会话的标签未注册
        let (_, other_a, other_b) = mint_with(2, 2_000);
        assert_ne!(other_a, tag_a);
        assert_eq!(reg.lookup(&other_a), None);
        assert_eq!(reg.lookup(&other_b), None);
        // 过期票据:同 sid 不同过期时刻 ⇒ 标签绑定失败(过期票签找不到活会话)
        let (_, expired_a, expired_b) = mint_with(1, 1_000);
        assert_ne!(expired_a, tag_a);
        assert_eq!(reg.lookup(&expired_a), None);
        assert_eq!(reg.lookup(&expired_b), None);
        // 伪造:随机 16B 及畸形长度一律 None,不 panic
        let mut forged = [0u8; 16];
        StdRng::seed_from_u64(99).fill_bytes(&mut forged);
        assert_ne!(forged, tag_a);
        assert_eq!(reg.lookup(&forged), None);
        assert_eq!(reg.lookup(&tag_a[..15]), None);
        assert_eq!(reg.lookup(&[0u8; 17]), None);
    }

    #[test]
    fn bucket_refills_with_injected_clock() {
        // 1000 kbps = 125_000 B/s,满桶容量即 1 秒配额
        let mut b = TokenBucket::new(1000);
        let t0 = Instant::now();
        assert!(b.try_take(125_000, t0)); // 满桶起步,一次取干
        assert!(!b.try_take(1, t0)); // 空桶拒绝(丢弃式)
        assert!(b.try_take(125_000, t0 + Duration::from_secs(1))); // 推进 1s 恰好补满
        assert!(b.try_take(31_250, t0 + Duration::from_millis(1_250))); // 250ms 补 31_250,取走
        assert!(!b.try_take(1, t0 + Duration::from_millis(1_250))); // 余额归零
    }

    #[test]
    fn bucket_drops_without_touching_balance_when_short() {
        let mut b = TokenBucket::new(1000);
        let t0 = Instant::now();
        assert!(b.try_take(124_999, t0));
        assert!(!b.try_take(2, t0)); // 差 1 字节,拒绝且不动余额
        assert!(b.try_take(1, t0)); // 原有余额仍可取
    }

    #[test]
    fn bucket_set_rate_takes_effect_immediately() {
        let t0 = Instant::now();
        // 调高:250ms 窗口按新速率 2000kbps=250_000B/s 补 62_500
        let mut up = TokenBucket::new(1000);
        assert!(up.try_take(125_000, t0)); // 取干旧桶
        up.set_rate(2000, t0);
        assert!(up.try_take(62_500, t0 + Duration::from_millis(250)));
        assert!(!up.try_take(1, t0 + Duration::from_millis(250)));
        // 调低:满桶 250_000 即时截断到新容量 125_000
        let mut down = TokenBucket::new(2000);
        down.set_rate(1000, t0);
        assert!(down.try_take(125_000, t0));
        assert!(!down.try_take(1, t0));
    }

    #[test]
    fn sweep_idle_removes_only_timed_out_sessions() {
        let t0 = Instant::now();
        let (ticket_s, a_s, b_s) = mint_with(1, 9_999);
        let (ticket_f, a_f, b_f) = mint_with(2, 9_999);
        let mut reg = Registry::new();
        reg.insert(session(ticket_s.sid, "acc", 1000, t0), a_s, b_s);
        // 距 sweep 时刻恰好 120s:不超时(阈值取严格大于)
        reg.insert(
            session(ticket_f.sid, "acc", 1000, t0 + Duration::from_secs(1)),
            a_f,
            b_f,
        );

        let swept = reg.sweep_idle(t0 + Duration::from_secs(121));
        assert_eq!(swept, vec![ticket_s.sid]);
        // 活会话原样保留;超时会话双 tag 失效
        assert!(reg.get_mut(ticket_f.sid).is_some());
        assert!(reg.get_mut(ticket_s.sid).is_none());
        assert_eq!(reg.lookup(&a_f), Some((ticket_f.sid, 0)));
        assert_eq!(reg.lookup(&a_s), None);
        assert_eq!(reg.lookup(&b_s), None);
        assert_eq!(reg.count_for_account("acc"), 1);
        // 再过 1s,原"恰好 120s"的会话也超时
        let swept2 = reg.sweep_idle(t0 + Duration::from_secs(122));
        assert_eq!(swept2, vec![ticket_f.sid]);
        assert_eq!(reg.lookup(&b_f), None);
        assert_eq!(reg.count_for_account("acc"), 0);
    }

    #[test]
    fn count_for_account_counts_only_that_account() {
        let t0 = Instant::now();
        let (t1, a1, b1) = mint_with(1, 9_999);
        let (t2, a2, b2) = mint_with(2, 9_999);
        let (t3, a3, b3) = mint_with(3, 9_999);
        let mut reg = Registry::new();
        reg.insert(session(t1.sid, "alice", 1000, t0), a1, b1);
        reg.insert(session(t2.sid, "alice", 1000, t0), a2, b2);
        reg.insert(session(t3.sid, "bob", 1000, t0), a3, b3);
        assert_eq!(reg.count_for_account("alice"), 2);
        assert_eq!(reg.count_for_account("bob"), 1);
        assert_eq!(reg.count_for_account("carol"), 0);
        assert!(reg.remove(t1.sid).is_some());
        assert_eq!(reg.count_for_account("alice"), 1);
    }

    #[test]
    fn remove_drops_session_and_both_tags() {
        let (mut reg, ticket, tag_a, tag_b) = registry_with_one(1, 9_999);
        let removed = reg.remove(ticket.sid);
        assert_eq!(removed.map(|s| s.ticket.sid), Some(ticket.sid));
        assert!(reg.remove(ticket.sid).is_none()); // 二次移除 None
        assert_eq!(reg.lookup(&tag_a), None);
        assert_eq!(reg.lookup(&tag_b), None);
        assert!(reg.get_mut(ticket.sid).is_none());
    }

    #[test]
    fn get_mut_allows_in_place_mutation() {
        let (mut reg, ticket, _, _) = registry_with_one(1, 9_999);
        reg.get_mut(ticket.sid).unwrap().bytes_accounted += 1500;
        assert_eq!(reg.get_mut(ticket.sid).unwrap().bytes_accounted, 1500);
        assert!(reg.get_mut(ticket.sid + 1).is_none()); // 未注册 sid → None
    }
}
