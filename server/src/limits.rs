//! Abuse protection: password brute-force throttling, per-IP connection
//! limits and token buckets for message and media rates.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tracing::{info, warn};

/// Failed password attempts allowed from one address within `FAIL_WINDOW`.
const MAX_FAILURES: u32 = 5;
const FAIL_WINDOW: Duration = Duration::from_secs(10 * 60);
/// How long an address stays blocked after too many failures.
const BLOCK_TIME: Duration = Duration::from_secs(15 * 60);
/// Above this many tracked addresses, expired entries are purged.
const PURGE_THRESHOLD: usize = 1024;
/// Wrong passwords allowed server-wide, from all addresses together:
/// `GLOBAL_FAIL_BURST` at once, refilled at `GLOBAL_FAIL_PER_SEC`
/// (about 100 per 10 minutes, 14 000 a day). Per-address blocking does not
/// stop a botnet guessing from thousands of addresses; this does. When the
/// budget is used up the server is "under attack": only addresses that
/// already passed the same check since startup get their password checked.
const GLOBAL_FAIL_BURST: f64 = 100.0;
const GLOBAL_FAIL_PER_SEC: f64 = 100.0 / 600.0;
/// Remembered addresses that logged in successfully, per password kind.
const MAX_TRUSTED: usize = 4096;

/// Key for per-address accounting. An IPv6 user usually controls a whole
/// /64, so counting single IPv6 addresses would be trivial to bypass.
pub fn ip_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let s = v6.segments();
                IpAddr::V6(std::net::Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
        },
    }
}

/// Compares secrets in constant time. Hashing first also hides the length.
pub fn secret_eq(a: &str, b: &str) -> bool {
    let (ha, hb) = (Sha256::digest(a.as_bytes()), Sha256::digest(b.as_bytes()));
    ha.iter().zip(hb.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

struct FailEntry {
    count: u32,
    window_start: Instant,
    blocked_until: Option<Instant>,
}

/// Which password is being checked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Secret {
    Server,
    Admin,
}

/// Wrong password attempts per address, shared by the join password
/// and the admin password, plus the server-wide budget.
pub struct AuthFailures {
    map: HashMap<IpAddr, FailEntry>,
    global: TokenBucket,
    under_attack: bool,
    /// Addresses that passed the server / admin password since startup.
    trusted: [HashSet<IpAddr>; 2],
}

impl AuthFailures {
    pub fn new(now: Instant) -> Self {
        Self {
            map: HashMap::new(),
            global: TokenBucket::new(GLOBAL_FAIL_BURST, GLOBAL_FAIL_PER_SEC, now),
            under_attack: false,
            trusted: [HashSet::new(), HashSet::new()],
        }
    }

    /// Whether the password from `ip` may be checked at all. If not, the
    /// caller answers "wrong password" without looking at it, so guessing
    /// gets no information.
    pub fn may_check(&mut self, ip: IpAddr, secret: Secret, now: Instant) -> bool {
        if self.is_blocked(ip, now) {
            return false;
        }
        if self.under_attack {
            if self.global.available(now) < 1.0 {
                return self.trusted[secret as usize].contains(&ip_key(ip));
            }
            self.under_attack = false;
            info!("wrong password rate is back to normal, checking passwords from all addresses again");
        }
        true
    }

    pub fn is_blocked(&self, ip: IpAddr, now: Instant) -> bool {
        self.map.get(&ip_key(ip)).and_then(|e| e.blocked_until).is_some_and(|t| now < t)
    }

    /// Records a failure. Returns true if the address just got blocked.
    pub fn record_failure(&mut self, ip: IpAddr, now: Instant) -> bool {
        if !self.global.try_take(1.0, now) && !self.under_attack {
            self.under_attack = true;
            warn!(
                "too many wrong passwords server-wide (possible distributed guessing): \
                 until it calms down only addresses that already logged in can log in"
            );
        }
        if self.map.len() > PURGE_THRESHOLD {
            self.map.retain(|_, e| match e.blocked_until {
                Some(t) => now < t,
                None => now.duration_since(e.window_start) < FAIL_WINDOW,
            });
        }
        let e = self.map.entry(ip_key(ip)).or_insert(FailEntry { count: 0, window_start: now, blocked_until: None });
        if e.blocked_until.is_some_and(|t| now >= t) || now.duration_since(e.window_start) >= FAIL_WINDOW {
            *e = FailEntry { count: 0, window_start: now, blocked_until: None };
        }
        e.count += 1;
        if e.count >= MAX_FAILURES && e.blocked_until.is_none() {
            e.blocked_until = Some(now + BLOCK_TIME);
            return true;
        }
        false
    }

    pub fn record_success(&mut self, ip: IpAddr, secret: Secret) {
        let key = ip_key(ip);
        if self.map.get(&key).is_some_and(|e| e.blocked_until.is_none()) {
            self.map.remove(&key);
        }
        let trusted = &mut self.trusted[secret as usize];
        if trusted.len() < MAX_TRUSTED {
            trusted.insert(key);
        }
    }

    /// Unblocks an address from the console. Returns true if it was tracked.
    pub fn clear(&mut self, ip: IpAddr) -> bool {
        self.map.remove(&ip_key(ip)).is_some()
    }
}

/// Counts open connections per address, including ones still
/// in the handshake. A slot is released when its guard is dropped.
#[derive(Clone, Default)]
pub struct ConnCounter {
    inner: Arc<Mutex<HashMap<IpAddr, u32>>>,
}

pub struct ConnSlot {
    counter: ConnCounter,
    key: IpAddr,
}

impl ConnCounter {
    pub fn try_acquire(&self, ip: IpAddr, max: u32) -> Option<ConnSlot> {
        let key = ip_key(ip);
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let n = map.entry(key).or_insert(0);
        if *n >= max {
            return None;
        }
        *n += 1;
        Some(ConnSlot { counter: self.clone(), key })
    }
}

impl Drop for ConnSlot {
    fn drop(&mut self) {
        let mut map = self.counter.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = map.get_mut(&self.key) {
            *n -= 1;
            if *n == 0 {
                map.remove(&self.key);
            }
        }
    }
}

/// Classic token bucket: `capacity` tokens, refilled at `rate` per second.
pub struct TokenBucket {
    capacity: f64,
    rate: f64,
    tokens: f64,
    last: Instant,
}

impl TokenBucket {
    pub fn new(capacity: f64, rate: f64, now: Instant) -> Self {
        Self { capacity, rate, tokens: capacity, last: now }
    }

    fn refill(&mut self, now: Instant) {
        let dt = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = self.last.max(now);
        self.tokens = (self.tokens + dt * self.rate).min(self.capacity);
    }

    pub fn available(&mut self, now: Instant) -> f64 {
        self.refill(now);
        self.tokens
    }

    pub fn try_take(&mut self, cost: f64, now: Instant) -> bool {
        self.refill(now);
        if self.tokens >= cost {
            self.tokens -= cost;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn ipv6_grouped_by_64() {
        assert_eq!(ip_key(ip("2001:db8:1:2:aaaa::1")), ip_key(ip("2001:db8:1:2:bbbb::2")));
        assert_ne!(ip_key(ip("2001:db8:1:2::1")), ip_key(ip("2001:db8:1:3::1")));
        assert_eq!(ip_key(ip("::ffff:1.2.3.4")), ip("1.2.3.4"));
        assert_eq!(ip_key(ip("1.2.3.4")), ip("1.2.3.4"));
    }

    #[test]
    fn secret_comparison() {
        assert!(secret_eq("hunter2", "hunter2"));
        assert!(!secret_eq("hunter2", "hunter3"));
        assert!(!secret_eq("hunter2", ""));
    }

    #[test]
    fn blocks_after_failures_and_expires() {
        let mut f = AuthFailures::new(Instant::now());
        let a = ip("10.0.0.1");
        let t0 = Instant::now();
        for i in 1..MAX_FAILURES {
            assert!(!f.record_failure(a, t0), "attempt {i}");
            assert!(!f.is_blocked(a, t0));
        }
        assert!(f.record_failure(a, t0));
        assert!(f.is_blocked(a, t0));
        assert!(!f.is_blocked(ip("10.0.0.2"), t0));
        // Success does not lift an active block.
        f.record_success(a, Secret::Server);
        assert!(f.is_blocked(a, t0));
        assert!(!f.is_blocked(a, t0 + BLOCK_TIME));
        // After the block expires the counter starts over.
        assert!(!f.record_failure(a, t0 + BLOCK_TIME));
    }

    #[test]
    fn failures_outside_window_do_not_add_up() {
        let mut f = AuthFailures::new(Instant::now());
        let a = ip("10.0.0.1");
        let t0 = Instant::now();
        for _ in 1..MAX_FAILURES {
            f.record_failure(a, t0);
        }
        assert!(!f.record_failure(a, t0 + FAIL_WINDOW));
        assert!(!f.is_blocked(a, t0 + FAIL_WINDOW));
    }

    #[test]
    fn success_resets_counter() {
        let mut f = AuthFailures::new(Instant::now());
        let a = ip("10.0.0.1");
        let t0 = Instant::now();
        for _ in 1..MAX_FAILURES {
            f.record_failure(a, t0);
        }
        f.record_success(a, Secret::Server);
        assert!(!f.record_failure(a, t0));
    }

    #[test]
    fn global_budget_protects_against_many_addresses() {
        let t0 = Instant::now();
        let mut f = AuthFailures::new(t0);
        let friend = ip("192.168.0.10");
        f.record_success(friend, Secret::Server);
        // A botnet: every address fails once, none gets blocked by itself.
        for i in 0..GLOBAL_FAIL_BURST as u32 {
            let a = IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + i));
            assert!(f.may_check(a, Secret::Server, t0));
            f.record_failure(a, t0);
        }
        let stranger = ip("172.16.0.1");
        f.record_failure(stranger, t0);
        // Under attack: strangers are not checked, known addresses still are.
        assert!(!f.may_check(stranger, Secret::Server, t0));
        assert!(!f.may_check(ip("172.16.0.2"), Secret::Server, t0));
        assert!(f.may_check(friend, Secret::Server, t0));
        // Trust is per password: the friend never passed the admin password.
        assert!(!f.may_check(friend, Secret::Admin, t0));
        // Once the budget refills, everybody is checked again.
        let later = t0 + Duration::from_secs_f64(2.0 / GLOBAL_FAIL_PER_SEC);
        assert!(f.may_check(ip("172.16.0.2"), Secret::Server, later));
    }

    #[test]
    fn conn_counter_limits_and_releases() {
        let c = ConnCounter::default();
        let a = ip("10.0.0.1");
        let s1 = c.try_acquire(a, 2).unwrap();
        let _s2 = c.try_acquire(a, 2).unwrap();
        assert!(c.try_acquire(a, 2).is_none());
        assert!(c.try_acquire(ip("10.0.0.2"), 2).is_some());
        drop(s1);
        assert!(c.try_acquire(a, 2).is_some());
    }

    #[test]
    fn token_bucket_refills() {
        let t0 = Instant::now();
        let mut b = TokenBucket::new(3.0, 1.0, t0);
        assert!(b.try_take(1.0, t0));
        assert!(b.try_take(1.0, t0));
        assert!(b.try_take(1.0, t0));
        assert!(!b.try_take(1.0, t0));
        assert!(b.try_take(1.0, t0 + Duration::from_secs(1)));
        assert!(!b.try_take(1.0, t0 + Duration::from_secs(1)));
        // Never exceeds capacity after a long pause.
        let later = t0 + Duration::from_secs(100);
        assert!(b.try_take(3.0, later));
        assert!(!b.try_take(1.0, later));
    }
}
