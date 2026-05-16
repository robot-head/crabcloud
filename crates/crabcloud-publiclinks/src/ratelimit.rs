//! In-memory windowed rate limiting for public links. Two flavors:
//! - Per-token password-unlock attempts: 10 per hour per token.
//! - Per-IP file-drop uploads: 60 per hour per IP.
//!
//! MVP single-node scope; state vanishes on process restart. Documented
//! limitation — a later SP can swap for durable counters if multi-node.

use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const PASSWORD_ATTEMPTS_PER_HOUR: u32 = 10;
pub const UPLOAD_ATTEMPTS_PER_HOUR: u32 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitDecision {
    Allowed,
    Throttled { retry_after_secs: u64 },
}

#[derive(Debug, Clone, Copy)]
struct AttemptLog {
    window_start: Instant,
    count: u32,
}

#[derive(Clone)]
pub struct RateLimiter {
    inner: Arc<RateLimiterInner>,
}

struct RateLimiterInner {
    password_attempts: DashMap<String, AttemptLog>,
    upload_attempts: DashMap<String, AttemptLog>,
    window: Duration,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::with_window(Duration::from_secs(3600))
    }

    pub fn with_window(window: Duration) -> Self {
        Self {
            inner: Arc::new(RateLimiterInner {
                password_attempts: DashMap::new(),
                upload_attempts: DashMap::new(),
                window,
            }),
        }
    }

    pub fn check_password_attempt(&self, token: &str) -> RateLimitDecision {
        self.check(
            &self.inner.password_attempts,
            token,
            PASSWORD_ATTEMPTS_PER_HOUR,
        )
    }

    pub fn check_upload(&self, ip: &str) -> RateLimitDecision {
        self.check(&self.inner.upload_attempts, ip, UPLOAD_ATTEMPTS_PER_HOUR)
    }

    fn check(
        &self,
        bucket: &DashMap<String, AttemptLog>,
        key: &str,
        cap: u32,
    ) -> RateLimitDecision {
        let now = Instant::now();
        let window = self.inner.window;
        let mut entry = bucket.entry(key.to_string()).or_insert(AttemptLog {
            window_start: now,
            count: 0,
        });
        if now.duration_since(entry.window_start) >= window {
            entry.window_start = now;
            entry.count = 0;
        }
        if entry.count >= cap {
            let elapsed = now.duration_since(entry.window_start);
            let retry_after_secs = window.saturating_sub(elapsed).as_secs().max(1);
            return RateLimitDecision::Throttled { retry_after_secs };
        }
        entry.count += 1;
        RateLimitDecision::Allowed
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_ten_password_attempts_allowed_eleventh_throttled() {
        let rl = RateLimiter::new();
        for _ in 0..PASSWORD_ATTEMPTS_PER_HOUR {
            assert!(matches!(
                rl.check_password_attempt("tok"),
                RateLimitDecision::Allowed
            ));
        }
        assert!(matches!(
            rl.check_password_attempt("tok"),
            RateLimitDecision::Throttled { .. }
        ));
    }

    #[test]
    fn upload_cap_higher_than_password_cap() {
        let rl = RateLimiter::new();
        for _ in 0..UPLOAD_ATTEMPTS_PER_HOUR {
            assert!(matches!(
                rl.check_upload("1.2.3.4"),
                RateLimitDecision::Allowed
            ));
        }
        assert!(matches!(
            rl.check_upload("1.2.3.4"),
            RateLimitDecision::Throttled { .. }
        ));
    }

    #[test]
    fn distinct_keys_have_independent_counters() {
        let rl = RateLimiter::new();
        for _ in 0..PASSWORD_ATTEMPTS_PER_HOUR {
            assert!(matches!(
                rl.check_password_attempt("a"),
                RateLimitDecision::Allowed
            ));
        }
        assert!(matches!(
            rl.check_password_attempt("b"),
            RateLimitDecision::Allowed
        ));
    }

    #[test]
    fn window_resets_with_short_window() {
        let rl = RateLimiter::with_window(Duration::from_millis(50));
        for _ in 0..PASSWORD_ATTEMPTS_PER_HOUR {
            rl.check_password_attempt("t");
        }
        assert!(matches!(
            rl.check_password_attempt("t"),
            RateLimitDecision::Throttled { .. }
        ));
        std::thread::sleep(Duration::from_millis(80));
        assert!(matches!(
            rl.check_password_attempt("t"),
            RateLimitDecision::Allowed
        ));
    }
}
