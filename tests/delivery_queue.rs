use std::sync::{Arc, Mutex};

use picobot::channels::adapter::{OutboundMessage, OutboundSender};
use picobot::delivery::queue::{DeliveryQueue, DeliveryQueueConfig};
use picobot::delivery::tracking::{DeliveryStatus, DeliveryTracker};

struct StubSender {
    attempts: Arc<Mutex<usize>>,
}

#[async_trait::async_trait]
impl OutboundSender for StubSender {
    fn sender_id(&self) -> &str {
        "stub"
    }

    fn supports_streaming(&self) -> bool {
        false
    }

    async fn send(
        &self,
        _msg: OutboundMessage,
    ) -> Result<picobot::channels::adapter::DeliveryId, anyhow::Error> {
        let mut attempts = self.attempts.lock().unwrap();
        *attempts += 1;
        if *attempts < 2 {
            Err(anyhow::anyhow!("fail"))
        } else {
            Ok("delivered".to_string())
        }
    }

    async fn stream_token(&self, _session_id: &str, _token: &str) -> Result<(), anyhow::Error> {
        Ok(())
    }
}

#[tokio::test]
async fn delivery_queue_retries_and_succeeds() {
    let tracker = DeliveryTracker::new();
    let queue = DeliveryQueue::new(
        tracker.clone(),
        DeliveryQueueConfig {
            max_attempts: 3,
            base_backoff: std::time::Duration::from_millis(10),
            max_backoff: std::time::Duration::from_millis(20),
        },
    );
    let sender = Arc::new(StubSender {
        attempts: Arc::new(Mutex::new(0)),
    });
    let worker = queue.clone();
    tokio::spawn(async move {
        worker.worker_loop(sender).await;
    });

    let delivery_id = queue
        .enqueue(OutboundMessage {
            channel_id: "whatsapp".to_string(),
            user_id: "user".to_string(),
            text: "hello".to_string(),
        })
        .await;

    for _ in 0..10 {
        if let Some(record) = tracker.get(&delivery_id) {
            if record.status == DeliveryStatus::Sent {
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }

    let record = tracker.get(&delivery_id).expect("record");
    assert_eq!(record.status, DeliveryStatus::Sent);
}
