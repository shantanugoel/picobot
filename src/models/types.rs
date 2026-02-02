use serde::{Deserialize, Serialize};

use crate::tools::traits::ToolSpec;

pub type ModelId = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: ModelId,
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        content: String,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

impl Message {
    pub fn system<S: Into<String>>(content: S) -> Self {
        Self::System {
            content: content.into(),
        }
    }

    pub fn user<S: Into<String>>(content: S) -> Self {
        Self::User {
            content: content.into(),
        }
    }

    pub fn assistant<S: Into<String>>(content: S) -> Self {
        Self::Assistant {
            content: content.into(),
        }
    }

    pub fn tool<S: Into<String>, T: Into<String>>(tool_call_id: T, content: S) -> Self {
        Self::Tool {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModelEvent {
    Token(String),
    ToolCall(ToolInvocation),
    Done(ModelResponse),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModelResponse {
    Text(String),
    ToolCalls(Vec<ToolInvocation>),
}
