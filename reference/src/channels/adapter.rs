use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};

use crate::channels::permissions::ChannelPermissionProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelType {
    Websocket,
    Whatsapp,
    Api,
    Tui,
}

#[derive(Debug, Clone)]
pub struct InboundMessage {
    pub channel_id: String,
    pub user_id: String,
    pub text: String,
    pub message_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OutboundMessage {
    pub channel_id: String,
    pub user_id: String,
    pub text: String,
}

pub type DeliveryId = String;

#[async_trait]
pub trait InboundAdapter: Send + Sync {
    fn adapter_id(&self) -> &str;
    fn channel_type(&self) -> ChannelType;
    async fn subscribe(&self) -> Pin<Box<dyn Stream<Item = InboundMessage> + Send>>;
}

#[async_trait]
pub trait OutboundSender: Send + Sync {
    fn sender_id(&self) -> &str;
    fn supports_streaming(&self) -> bool;
    async fn send(&self, msg: OutboundMessage) -> Result<DeliveryId, anyhow::Error>;
    async fn stream_token(&self, session_id: &str, token: &str) -> Result<(), anyhow::Error>;
}

#[derive(Clone)]
pub struct Channel {
    pub id: String,
    pub channel_type: ChannelType,
    pub inbound: std::sync::Arc<dyn InboundAdapter>,
    pub outbound: std::sync::Arc<dyn OutboundSender>,
    pub permission_profile: ChannelPermissionProfile,
}
