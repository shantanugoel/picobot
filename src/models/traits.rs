use async_trait::async_trait;

use crate::models::types::{ModelEvent, ModelInfo, ModelRequest, ModelResponse};

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("Model request failed: {0}")]
    RequestFailed(String),
    #[error("Model response invalid: {0}")]
    InvalidResponse(String),
}

#[async_trait]
pub trait Model: Send + Sync {
    fn info(&self) -> ModelInfo;
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, ModelError>;
    async fn stream(&self, req: ModelRequest) -> Result<Vec<ModelEvent>, ModelError>;
}
