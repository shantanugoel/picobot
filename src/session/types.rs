use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::kernel::permissions::CapabilitySet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub channel_type: String,
    pub channel_id: String,
    pub user_id: String,
    pub permissions: CapabilitySet,
    pub created_at: DateTime<Utc>,
    pub last_active: DateTime<Utc>,
    pub state: SessionState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionState {
    Active,
    AwaitingPermission { tool: String, request_id: String },
    Idle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub message_type: MessageType,
    pub content: String,
    pub tool_call_id: Option<String>,
    pub seq_order: i64,
    pub token_estimate: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MessageType {
    System,
    User,
    Assistant,
    AssistantToolCalls,
    Tool,
}

impl MessageType {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageType::System => "system",
            MessageType::User => "user",
            MessageType::Assistant => "assistant",
            MessageType::AssistantToolCalls => "assistant_tool_calls",
            MessageType::Tool => "tool",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "system" => Some(MessageType::System),
            "user" => Some(MessageType::User),
            "assistant" => Some(MessageType::Assistant),
            "assistant_tool_calls" => Some(MessageType::AssistantToolCalls),
            "tool" => Some(MessageType::Tool),
            _ => None,
        }
    }
}
