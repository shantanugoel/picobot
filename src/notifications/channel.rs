use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct NotificationRequest {
    pub user_id: String,
    pub channel_id: String,
    pub message: String,
}

#[async_trait]
pub trait NotificationChannel: Send + Sync {
    fn channel_id(&self) -> &str;
    async fn send(&self, request: NotificationRequest) -> Result<(), anyhow::Error>;
}
