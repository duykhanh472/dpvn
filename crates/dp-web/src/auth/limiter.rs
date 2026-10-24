//! In-process sliding window rate limiting for sensitive authentication endpoints (§12.5).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// In-memory sliding window rate limiter.
#[derive(Debug, Default)]
pub struct RateLimiter {
    buckets: RwLock<HashMap<String, Vec<Instant>>>,
}

impl RateLimiter {
    /// Create a new empty rate limiter.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buckets: RwLock::new(HashMap::new()),
        }
    }

    /// Check and record an attempt against the rate limit.
    ///
    /// Returns `true` if allowed, `false` if rate limit is exceeded.
    pub fn check_and_record(&self, key: &str, limit: usize, window: Duration) -> bool {
        let now = Instant::now();
        let mut buckets = self.buckets.write().unwrap();

        let entries = buckets.entry(key.to_string()).or_default();
        // Remove expired entries
        entries.retain(|&timestamp| now.duration_since(timestamp) < window);

        if entries.len() >= limit {
            false
        } else {
            entries.push(now);
            true
        }
    }

    /// Check registration rate limit: 5 per hour per IP.
    pub fn check_registration(&self, ip: IpAddr) -> bool {
        self.check_and_record(&format!("reg:{ip}"), 5, Duration::from_secs(60 * 60))
    }

    /// Check login rate limit: 10 per 15 minutes per IP, and 5 per username.
    pub fn check_login(&self, ip: IpAddr, username: &str) -> bool {
        let ip_allowed =
            self.check_and_record(&format!("login_ip:{ip}"), 10, Duration::from_secs(15 * 60));
        let user_allowed = self.check_and_record(
            &format!("login_user:{}", username.to_lowercase()),
            5,
            Duration::from_secs(15 * 60),
        );
        ip_allowed && user_allowed
    }
}
