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

    pub fn queue(&self) -> &NotificationQueue {
        &self.queue
    }

    pub async fn enqueue(&self, request: NotificationRequest) -> String {
        self.queue.enqueue(request).await
    }

    pub async fn worker_loop(&self) {
        loop {
            let mut item = self.queue.pop().await;
            self.queue
                .record_status(&item.id, NotificationStatus::Sending, item.attempts, None)
                .await;
            match self.channel.send(item.request.clone()).await {
                Ok(_) => {
                    self.queue
                        .record_status(&item.id, NotificationStatus::Sent, item.attempts + 1, None)
                        .await;
                }
                Err(err) => {
                    item.attempts += 1;
                    let err_text = err.to_string();
                    if item.attempts >= self.queue.config().max_attempts {
                        self.queue
                            .record_status(
                                &item.id,
                                NotificationStatus::Failed,
                                item.attempts,
                                Some(err_text),
                            )
                            .await;
                        continue;
                    }
                    self.queue
                        .record_status(
                            &item.id,
                            NotificationStatus::Pending,
                            item.attempts,
                            Some(err_text),
                        )
                        .await;
                    self.queue.retry(item).await;
                }
            }
        }
    }
}
