use std::sync::Arc;

use crate::notifications::channel::{NotificationChannel, NotificationRequest};
use crate::notifications::queue::{NotificationQueue, NotificationStatus};

#[derive(Clone)]
pub struct NotificationService {
    queue: NotificationQueue,
    channel: Arc<dyn NotificationChannel>,
}

impl NotificationService {
    pub fn new(queue: NotificationQueue, channel: Arc<dyn NotificationChannel>) -> Self {
        Self { queue, channel }
    }

    pub async fn enqueue(&self, request: NotificationRequest) -> String {
        self.queue.enqueue(request).await
    }

    pub async fn worker_loop(&self) {
        loop {
            let mut item = self.queue.pop().await;
            let channel_id = self.channel.channel_id();
            if let Some(record) = self
                .queue
                .record_status(&item.id, NotificationStatus::Sending, item.attempts, None)
                .await
            {
                tracing::debug!(
                    event = "notification_status",
                    transport_channel_id = %channel_id,
                    channel_id = %record.channel_id,
                    user_id = %record.user_id,
                    status = ?record.status,
                    attempts = record.attempts,
                    "notification marked sending"
                );
            }
            match self.channel.send(item.request.clone()).await {
                Ok(_) => {
                    if let Some(record) = self
                        .queue
                        .record_status(&item.id, NotificationStatus::Sent, item.attempts + 1, None)
                        .await
                    {
                        tracing::debug!(
                            event = "notification_status",
                            transport_channel_id = %channel_id,
                            channel_id = %record.channel_id,
                            user_id = %record.user_id,
                            status = ?record.status,
                            attempts = record.attempts,
                            "notification sent"
                        );
                    }
                }
                Err(err) => {
                    item.attempts += 1;
                    let err_text = err.to_string();
                    if item.attempts >= self.queue.config().max_attempts {
                        if let Some(record) = self
                            .queue
                            .record_status(
                                &item.id,
                                NotificationStatus::Failed,
                                item.attempts,
                                Some(err_text),
                            )
                            .await
                        {
                            tracing::warn!(
                                event = "notification_failed",
                                transport_channel_id = %channel_id,
                                channel_id = %record.channel_id,
                                user_id = %record.user_id,
                                status = ?record.status,
                                attempts = record.attempts,
                                "notification delivery failed"
                            );
                        }
                        continue;
                    }
                    if let Some(record) = self
                        .queue
                        .record_status(
                            &item.id,
                            NotificationStatus::Pending,
                            item.attempts,
                            Some(err_text),
                        )
                        .await
                    {
                        tracing::debug!(
                            event = "notification_retry",
                            transport_channel_id = %channel_id,
                            channel_id = %record.channel_id,
                            user_id = %record.user_id,
                            status = ?record.status,
                            attempts = record.attempts,
                            "notification scheduled for retry"
                        );
                    }
                    self.queue.retry(item).await;
                }
            }
        }
    }
}
