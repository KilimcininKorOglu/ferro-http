//! Per-peer request rate limiting (the basic DDoS defense).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::config::RateLimitConfig;

/// Upper bound on tracked peers, capping memory under a many-distinct-IP flood.
const MAX_ENTRIES: usize = 100_000;

/// The outcome of a rate-limit check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny { retry_after_secs: u64 },
}

struct Entry {
    window_start: u64,
    count: u64,
    banned_until: u64,
}

/// Fixed-window per-peer rate limiter with temporary bans.
///
/// Not `Sync`: a multi-threaded caller wraps it in a mutex. The check is
/// O(log n) (one map lookup, a window comparison, a counter bump). Eviction of
/// expired entries is throttled to at most once per second and runs only when
/// the map is full, so the hot path never sweeps the map.
pub struct RateLimiter {
    config: RateLimitConfig,
    entries: BTreeMap<[u8; 16], Entry>,
    last_evict: u64,
}

impl RateLimiter {
    /// Creates a limiter from its configuration.
    pub fn new(config: RateLimitConfig) -> RateLimiter {
        RateLimiter {
            config,
            entries: BTreeMap::new(),
            last_evict: 0,
        }
    }

    /// Records a request from `peer` at `now` (Unix seconds) and decides whether
    /// to allow it.
    pub fn check(&mut self, peer: [u8; 16], now: u64) -> Decision {
        if !self.config.enabled {
            return Decision::Allow;
        }
        if let Some(entry) = self.entries.get_mut(&peer) {
            return Self::tick(entry, &self.config, now);
        }
        // New peer: make room if needed, then start tracking.
        if self.entries.len() >= MAX_ENTRIES {
            self.evict_expired(now);
            if self.entries.len() >= MAX_ENTRIES {
                // Still full of active peers: fail open rather than grow without
                // bound. Per-peer floods (already tracked) stay limited.
                return Decision::Allow;
            }
        }
        let mut entry = Entry {
            window_start: now,
            count: 0,
            banned_until: 0,
        };
        let decision = Self::tick(&mut entry, &self.config, now);
        self.entries.insert(peer, entry);
        decision
    }

    fn tick(entry: &mut Entry, config: &RateLimitConfig, now: u64) -> Decision {
        if now < entry.banned_until {
            return Decision::Deny {
                retry_after_secs: entry.banned_until - now,
            };
        }
        // Start a fresh window when the fixed window elapsed, or when a ban just
        // expired (otherwise the still-high count would immediately re-ban while
        // the window outlives the ban).
        if entry.banned_until != 0 || now.saturating_sub(entry.window_start) >= config.window_secs {
            entry.window_start = now;
            entry.count = 0;
            entry.banned_until = 0;
        }
        entry.count += 1;
        if entry.count > config.requests {
            entry.banned_until = now + config.ban_secs;
            return Decision::Deny {
                retry_after_secs: config.ban_secs,
            };
        }
        Decision::Allow
    }

    fn evict_expired(&mut self, now: u64) {
        if now <= self.last_evict {
            return; // throttle: at most once per second
        }
        self.last_evict = now;
        let window = self.config.window_secs;
        let stale: Vec<[u8; 16]> = self
            .entries
            .iter()
            .filter(|(_, e)| now >= e.banned_until && now.saturating_sub(e.window_start) >= window)
            .map(|(k, _)| *k)
            .collect();
        for key in stale {
            self.entries.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(enabled: bool) -> RateLimitConfig {
        RateLimitConfig {
            enabled,
            requests: 3,
            window_secs: 10,
            ban_secs: 60,
        }
    }

    const PEER: [u8; 16] = [10, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

    #[test]
    fn disabled_always_allows() {
        let mut limiter = RateLimiter::new(config(false));
        for _ in 0..1000 {
            assert_eq!(limiter.check(PEER, 0), Decision::Allow);
        }
    }

    #[test]
    fn allows_up_to_limit_then_bans() {
        let mut limiter = RateLimiter::new(config(true));
        // 3 allowed in the window.
        assert_eq!(limiter.check(PEER, 0), Decision::Allow);
        assert_eq!(limiter.check(PEER, 0), Decision::Allow);
        assert_eq!(limiter.check(PEER, 0), Decision::Allow);
        // 4th exceeds and triggers a ban.
        assert_eq!(
            limiter.check(PEER, 0),
            Decision::Deny {
                retry_after_secs: 60
            }
        );
        // Still banned a few seconds later, with a shrinking retry-after.
        assert_eq!(
            limiter.check(PEER, 5),
            Decision::Deny {
                retry_after_secs: 55
            }
        );
    }

    #[test]
    fn recovers_after_ban_expires() {
        // Ban (5s) shorter than the window (100s): once the ban lifts, the
        // counter must reset instead of immediately re-banning inside the same
        // window. (A short window would mask this via the window reset.)
        let cfg = RateLimitConfig {
            enabled: true,
            requests: 3,
            window_secs: 100,
            ban_secs: 5,
        };
        let mut limiter = RateLimiter::new(cfg);
        for _ in 0..4 {
            limiter.check(PEER, 0); // the 4th request bans until t=5
        }
        assert_eq!(
            limiter.check(PEER, 3),
            Decision::Deny {
                retry_after_secs: 2
            }
        );
        // Ban expired at t=5; the window has not elapsed, but the count resets.
        assert_eq!(limiter.check(PEER, 6), Decision::Allow);
    }

    #[test]
    fn window_resets_counter() {
        let mut limiter = RateLimiter::new(config(true));
        assert_eq!(limiter.check(PEER, 0), Decision::Allow);
        assert_eq!(limiter.check(PEER, 0), Decision::Allow);
        // A new window (>= 10s later) resets the count, so 3 more are allowed.
        assert_eq!(limiter.check(PEER, 10), Decision::Allow);
        assert_eq!(limiter.check(PEER, 10), Decision::Allow);
        assert_eq!(limiter.check(PEER, 10), Decision::Allow);
    }
}
