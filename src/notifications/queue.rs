use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify};

use crate::notifications::channel::NotificationRequest;

#[derive(Debug, Clone)]
pub struct NotificationQueueConfig {
    pub max_attempts: usize,
    pub base_backoff: Duration,
    pub max_backoff: Duration,
    pub max_records: usize,
}

impl Default for NotificationQueueConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_backoff: Duration::from_millis(200),
            max_backoff: Duration::from_secs(5),
            max_records: 1000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NotificationRecord {
    pub id: String,
    pub user_id: String,
    pub channel_id: String,
    pub status: NotificationStatus,
    pub attempts: usize,
    pub last_error: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationStatus {
    Pending,
    Sending,
    Sent,
    Failed,
}

#[derive(Debug)]
pub struct QueueItem {
    pub id: String,
    pub request: NotificationRequest,
    pub attempts: usize,
}

#[derive(Debug, Default)]
struct QueueState {
    pending: VecDeque<QueueItem>,
}

#[derive(Debug, Default, Clone)]
pub struct NotificationQueue {
    state: Arc<Mutex<QueueState>>,
    notify: Arc<Notify>,
    records: Arc<Mutex<Vec<NotificationRecord>>>,
    config: NotificationQueueConfig,
}

impl NotificationQueue {
    pub fn new(config: NotificationQueueConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(QueueState::default())),
            notify: Arc::new(Notify::new()),
            records: Arc::new(Mutex::new(Vec::new())),
            config,
        }
    }

    pub async fn enqueue(&self, request: NotificationRequest) -> String {
        let id = format!(
            "{}:{}:{}",
            request.channel_id,
            request.user_id,
            uuid::Uuid::new_v4()
        );
        let record = NotificationRecord {
            id: id.clone(),
            user_id: request.user_id.clone(),
            channel_id: request.channel_id.clone(),
            status: NotificationStatus::Pending,
            attempts: 0,
            last_error: None,
            updated_at: chrono::Utc::now(),
        };
        let mut guard = self.records.lock().await;
        guard.push(record);
        prune_records(&mut guard, self.config.max_records);
        let mut state = self.state.lock().await;
        state.pending.push_back(QueueItem {
            id: id.clone(),
            request,
            attempts: 0,
        });
        self.notify.notify_one();
        id
    }

    pub async fn pop(&self) -> QueueItem {
        loop {
            if let Some(item) = {
                let mut guard = self.state.lock().await;
                guard.pending.pop_front()
            } {
                return item;
            }
            self.notify.notified().await;
        }
    }

    pub async fn record_status(
        &self,
        id: &str,
        status: NotificationStatus,
        attempts: usize,
        last_error: Option<String>,
    ) -> Option<NotificationRecord> {
        let mut guard = self.records.lock().await;
        if let Some(record) = guard.iter_mut().find(|record| record.id == id) {
            record.status = status;
            record.attempts = attempts;
            record.last_error = last_error;
            record.updated_at = chrono::Utc::now();
            return Some(record.clone());
        }
        None
    }

    pub async fn retry(&self, item: QueueItem) {
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

    pub fn config(&self) -> &NotificationQueueConfig {
        &self.config
    }
}

fn compute_backoff(attempt: usize, config: &NotificationQueueConfig) -> Duration {
    let exp = attempt.saturating_sub(1) as u32;
    let multiplier = 1u64.checked_shl(exp.min(10)).unwrap_or(u64::MAX);
    let base = config.base_backoff.as_millis() as u64;
    let backoff = base.saturating_mul(multiplier);
    let max = config.max_backoff.as_millis() as u64;
    Duration::from_millis(std::cmp::min(backoff, max))
}

fn prune_records(records: &mut Vec<NotificationRecord>, max_records: usize) {
    if max_records == 0 {
        records.clear();
        return;
    }
    if records.len() <= max_records {
        return;
    }
    let mut excess = records.len().saturating_sub(max_records);
    let mut kept = Vec::with_capacity(records.len());
    for record in records.drain(..) {
        if excess > 0
            && matches!(
                record.status,
                NotificationStatus::Sent | NotificationStatus::Failed
            )
        {
            excess -= 1;
        } else {
            kept.push(record);
        }
    }
    records.extend(kept);
    if records.len() > max_records {
        let drop_count = records.len() - max_records;
        records.drain(0..drop_count);
    }
}
