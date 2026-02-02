use async_trait::async_trait;
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
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocation {
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

#[async_trait]
pub trait Model: Send + Sync {
    fn info(&self) -> ModelInfo;
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, ModelError>;
    async fn stream(&self, req: ModelRequest) -> Result<Vec<ModelEvent>, ModelError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("Model request failed: {0}")]
    RequestFailed(String),
    #[error("Model response invalid: {0}")]
    InvalidResponse(String),
}
