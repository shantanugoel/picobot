use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify};

use crate::channels::adapter::{DeliveryId, OutboundMessage, OutboundSender};
use crate::delivery::tracking::{DeliveryRecord, DeliveryStatus, DeliveryTracker};

#[derive(Debug, Clone)]
pub struct DeliveryQueueConfig {
    pub max_attempts: usize,
    pub base_backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for DeliveryQueueConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_backoff: Duration::from_millis(200),
            max_backoff: Duration::from_secs(5),
        }
    }
}

#[derive(Debug)]
struct QueueItem {
    id: DeliveryId,
    message: OutboundMessage,
    attempts: usize,
}

#[derive(Debug, Default)]
struct QueueState {
    pending: VecDeque<QueueItem>,
}

#[derive(Debug, Default, Clone)]
pub struct DeliveryQueue {
    state: Arc<Mutex<QueueState>>,
    notify: Arc<Notify>,
    tracker: DeliveryTracker,
    config: DeliveryQueueConfig,
}

impl DeliveryQueue {
    pub fn new(tracker: DeliveryTracker, config: DeliveryQueueConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(QueueState::default())),
            notify: Arc::new(Notify::new()),
            tracker,
            config,
        }
    }

    pub fn tracker(&self) -> DeliveryTracker {
        self.tracker.clone()
    }

    pub async fn enqueue(&self, message: OutboundMessage) -> DeliveryId {
        let id = format!(
            "{}:{}:{}",
            message.channel_id,
            message.user_id,
            uuid::Uuid::new_v4()
        );
        let record = DeliveryRecord {
            id: id.clone(),
            channel_id: message.channel_id.clone(),
            user_id: message.user_id.clone(),
            status: DeliveryStatus::Pending,
            attempts: 0,
            last_error: None,
            updated_at: chrono::Utc::now(),
        };
        self.tracker.upsert(record);
        let mut state = self.state.lock().await;
        state.pending.push_back(QueueItem {
            id: id.clone(),
            message,
            attempts: 0,
        });
        self.notify.notify_one();
        id
    }

    pub async fn worker_loop(&self, outbound: Arc<dyn OutboundSender>) {
        loop {
            let item = {
                let mut guard = self.state.lock().await;
                guard.pending.pop_front()
            };
            let Some(mut item) = item else {
                self.notify.notified().await;
                continue;
            };

            self.tracker
                .update_status(&item.id, DeliveryStatus::Sending, item.attempts, None);

            match outbound.send(item.message.clone()).await {
                Ok(_) => {
                    self.tracker.update_status(
                        &item.id,
                        DeliveryStatus::Sent,
                        item.attempts + 1,
                        None,
                    );
                }
                Err(err) => {
                    item.attempts += 1;
                    let err_text = err.to_string();
                    if item.attempts >= self.config.max_attempts {
                        self.tracker.update_status(
                            &item.id,
                            DeliveryStatus::Failed,
                            item.attempts,
                            Some(err_text),
                        );
                        continue;
                    }
                    self.tracker.update_status(
                        &item.id,
                        DeliveryStatus::Pending,
                        item.attempts,
                        Some(err_text),
                    );
                    let backoff = compute_backoff(item.attempts, &self.config);
                    let state = Arc::clone(&self.state);
                    let notify = Arc::clone(&self.notify);
                    tokio::spawn(async move {
                        tokio::time::sleep(backoff).await;
                        let mut guard = state.lock().await;
                        guard.pending.push_back(item);
                        notify.notify_one();
                    });
                }
            }
        }
    }
}

fn compute_backoff(attempt: usize, config: &DeliveryQueueConfig) -> Duration {
    let exp = attempt.saturating_sub(1) as u32;
    let multiplier = 1u64.checked_shl(exp.min(10)).unwrap_or(u64::MAX);
    let base = config.base_backoff.as_millis() as u64;
    let backoff = base.saturating_mul(multiplier);
    let max = config.max_backoff.as_millis() as u64;
    Duration::from_millis(std::cmp::min(backoff, max))
}

#[cfg(test)]
mod tests {
    use super::{DeliveryQueueConfig, compute_backoff};
    use std::time::Duration;

    #[test]
    fn compute_backoff_caps_at_max() {
        let config = DeliveryQueueConfig {
            max_attempts: 3,
            base_backoff: Duration::from_millis(200),
            max_backoff: Duration::from_millis(400),
        };
        assert_eq!(compute_backoff(1, &config), Duration::from_millis(200));
        assert_eq!(compute_backoff(2, &config), Duration::from_millis(400));
        assert_eq!(compute_backoff(3, &config), Duration::from_millis(400));
    }
}
