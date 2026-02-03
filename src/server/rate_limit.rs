use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::config::RateLimitConfig;

#[derive(Debug, Clone)]
pub struct RateLimiter {
    state: Arc<Mutex<RateState>>,
    limit: u32,
    window: Duration,
}

#[derive(Debug)]
struct RateState {
    window_start: Instant,
    count: u32,
}

impl RateLimiter {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            state: Arc::new(Mutex::new(RateState {
                window_start: Instant::now(),
                count: 0,
            })),
            limit,
            window,
        }
    }

    pub fn from_config(config: &RateLimitConfig) -> Option<Self> {
        let per_minute = config.requests_per_minute?;
        let limit = std::cmp::max(1, per_minute);
        Some(Self::new(limit, Duration::from_secs(60)))
    }

    pub async fn check(&self) -> bool {
        let mut state = self.state.lock().await;
        let now = Instant::now();
        if now.duration_since(state.window_start) >= self.window {
            state.window_start = now;
            state.count = 0;
        }
        if state.count >= self.limit {
            return false;
        }
        state.count += 1;
        true
    }
}
