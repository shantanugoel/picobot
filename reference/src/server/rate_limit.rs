use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::config::RateLimitConfig;

#[derive(Debug, Clone)]
pub struct RateLimiter {
    state: Arc<Mutex<HashMap<String, RateState>>>,
    limit: u32,
    window: Duration,
    per_key: bool,
}

#[derive(Debug)]
struct RateState {
    window_start: Instant,
    count: u32,
}

impl RateLimiter {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            state: Arc::new(Mutex::new(HashMap::new())),
            limit,
            window,
            per_key: false,
        }
    }

    pub fn from_config(config: &RateLimitConfig) -> Option<Self> {
        let per_minute = config.requests_per_minute?;
        let limit = std::cmp::max(1, per_minute);
        let mut limiter = Self::new(limit, Duration::from_secs(60));
        limiter.per_key = config.per_key.unwrap_or(false);
        Some(limiter)
    }

    pub async fn check(&self) -> bool {
        self.check_key("global").await
    }

    pub async fn check_scoped(&self, key: &str) -> bool {
        let scoped_key = if self.per_key { key } else { "global" };
        self.check_key(scoped_key).await
    }

    async fn check_key(&self, key: &str) -> bool {
        let mut state = self.state.lock().await;
        let entry = state.entry(key.to_string()).or_insert(RateState {
            window_start: Instant::now(),
            count: 0,
        });
        let now = Instant::now();
        if now.duration_since(entry.window_start) >= self.window {
            entry.window_start = now;
            entry.count = 0;
        }
        if entry.count >= self.limit {
            return false;
        }
        entry.count += 1;
        true
    }
}
