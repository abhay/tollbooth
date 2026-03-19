use std::sync::Mutex;
use std::time::Instant;

use dashmap::DashMap;

/// In-process token bucket rate limiter keyed by client pubkey.
/// Best-effort soft limit, not stored in SQL.
/// Stale entries are evicted periodically to prevent unbounded memory growth.
/// Includes a global rate cap to prevent sybil bypass via key cycling.
pub struct RateLimiter {
    buckets: DashMap<String, Mutex<Bucket>>,
    capacity: u32,
    refill_per_second: f64,
    last_eviction: Mutex<Instant>,
    global_tokens: Mutex<f64>,
    global_capacity: f64,
    global_refill_per_second: f64,
    global_last_refill: Mutex<Instant>,
}

struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

impl RateLimiter {
    /// Evict entries older than this threshold.
    const EVICTION_TTL_SECS: u64 = 600; // 10 minutes
    /// Run eviction at most this often.
    const EVICTION_INTERVAL_SECS: u64 = 60;

    /// Create a rate limiter with the given requests-per-minute limit.
    /// The global cap is set to 10x the per-client rate, allowing reasonable
    /// aggregate throughput while preventing sybil bypass via key cycling.
    pub fn new(requests_per_minute: u32) -> Self {
        let global_capacity = (requests_per_minute as f64) * 10.0;
        let global_refill_per_second = global_capacity / 60.0;
        Self {
            buckets: DashMap::new(),
            capacity: requests_per_minute,
            refill_per_second: requests_per_minute as f64 / 60.0,
            last_eviction: Mutex::new(Instant::now()),
            global_tokens: Mutex::new(global_capacity),
            global_capacity,
            global_refill_per_second,
            global_last_refill: Mutex::new(Instant::now()),
        }
    }

    /// Check if a request from the given key is allowed. Returns true if allowed.
    /// Enforces both a per-key limit and a global aggregate limit.
    pub fn check(&self, key: &str) -> bool {
        self.maybe_evict();

        // Per-key rate limit
        let entry = self.buckets.entry(key.to_string()).or_insert_with(|| {
            Mutex::new(Bucket {
                tokens: self.capacity as f64,
                last_refill: Instant::now(),
            })
        });

        let mut bucket = entry.lock().unwrap();
        let now = Instant::now();
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens =
            (bucket.tokens + elapsed * self.refill_per_second).min(self.capacity as f64);
        bucket.last_refill = now;

        if bucket.tokens < 1.0 {
            return false;
        }

        // Global rate limit (prevents sybil bypass via key cycling)
        {
            let mut tokens = self.global_tokens.lock().unwrap();
            let mut last = self.global_last_refill.lock().unwrap();
            let global_elapsed = now.duration_since(*last).as_secs_f64();
            *tokens = (*tokens + global_elapsed * self.global_refill_per_second)
                .min(self.global_capacity);
            *last = now;
            if *tokens < 1.0 {
                return false;
            }
            *tokens -= 1.0;
        }

        bucket.tokens -= 1.0;
        true
    }

    /// Evict stale entries periodically to prevent unbounded memory growth.
    fn maybe_evict(&self) {
        let now = Instant::now();
        {
            let last = self.last_eviction.lock().unwrap();
            if now.duration_since(*last).as_secs() < Self::EVICTION_INTERVAL_SECS {
                return;
            }
        }
        // Update timestamp (best-effort, not critical if two threads race)
        *self.last_eviction.lock().unwrap() = now;

        let cutoff = std::time::Duration::from_secs(Self::EVICTION_TTL_SECS);
        self.buckets.retain(|_, v| {
            let bucket = v.get_mut().unwrap();
            now.duration_since(bucket.last_refill) < cutoff
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_up_to_capacity() {
        let limiter = RateLimiter::new(5);
        for _ in 0..5 {
            assert!(limiter.check("client1"));
        }
        assert!(!limiter.check("client1"));
    }

    #[test]
    fn separate_keys_independent() {
        let limiter = RateLimiter::new(2);
        assert!(limiter.check("a"));
        assert!(limiter.check("a"));
        assert!(!limiter.check("a"));
        // Different key still has full budget
        assert!(limiter.check("b"));
    }

    #[test]
    fn refills_over_time() {
        let limiter = RateLimiter::new(60); // 1 per second
        // Drain all tokens
        for _ in 0..60 {
            limiter.check("c");
        }
        assert!(!limiter.check("c"));
        // After sleeping ~1 second, should get 1 token back
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(limiter.check("c"));
    }
}
