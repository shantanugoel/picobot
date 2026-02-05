use std::sync::Arc;

use async_trait::async_trait;

use crate::channels::adapter::{OutboundMessage, OutboundSender};
use crate::channels::whatsapp::WhatsAppOutboundSender;
use crate::notifications::channel::{NotificationChannel, NotificationRequest};

#[derive(Clone)]
pub struct WhatsAppNotificationChannel {
    sender: Arc<WhatsAppOutboundSender>,
}

impl WhatsAppNotificationChannel {
    pub fn new(sender: Arc<WhatsAppOutboundSender>) -> Self {
        Self { sender }
    }
}

#[async_trait]
impl NotificationChannel for WhatsAppNotificationChannel {
    fn channel_id(&self) -> &str {
        "whatsapp"
    }

    async fn send(&self, request: NotificationRequest) -> Result<(), anyhow::Error> {
        let message = OutboundMessage {
            channel_id: request.channel_id,
            user_id: request.user_id,
            text: request.message,
        };
        let _ = self.sender.send(message).await?;
        Ok(())
    }
}
