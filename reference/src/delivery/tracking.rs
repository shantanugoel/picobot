use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::channels::adapter::DeliveryId;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Pending,
    Sending,
    Sent,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeliveryRecord {
    pub id: DeliveryId,
    pub channel_id: String,
    pub user_id: String,
    pub status: DeliveryStatus,
    pub attempts: usize,
    pub last_error: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Default, Clone)]
pub struct DeliveryTracker {
    inner: Arc<Mutex<HashMap<DeliveryId, DeliveryRecord>>>,
}

impl DeliveryTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&self, record: DeliveryRecord) {
        if let Ok(mut map) = self.inner.lock() {
            map.insert(record.id.clone(), record);
        }
    }

    pub fn update_status(
        &self,
        id: &DeliveryId,
        status: DeliveryStatus,
        attempts: usize,
        last_error: Option<String>,
    ) {
        if let Ok(mut map) = self.inner.lock()
            && let Some(record) = map.get_mut(id)
        {
            record.status = status;
            record.attempts = attempts;
            record.last_error = last_error;
            record.updated_at = chrono::Utc::now();
        }
    }

    pub fn get(&self, id: &DeliveryId) -> Option<DeliveryRecord> {
        self.inner.lock().ok()?.get(id).cloned()
    }

    pub fn snapshot(&self) -> Vec<DeliveryRecord> {
        self.inner
            .lock()
            .map(|map| map.values().cloned().collect())
            .unwrap_or_default()
    }
}
