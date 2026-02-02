use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::kernel::permissions::Permission;

pub type ToolOutput = serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub schema: serde_json::Value,
}

#[derive(Debug)]
pub struct ToolContext {
    pub working_dir: std::path::PathBuf,
    pub capabilities: std::sync::Arc<crate::kernel::permissions::CapabilitySet>,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn schema(&self) -> serde_json::Value;
    fn required_permissions(&self) -> Vec<Permission>;
    async fn execute(&self, ctx: &ToolContext, input: serde_json::Value) -> Result<ToolOutput, ToolError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("Permission denied for tool '{tool}': requires {required:?}")]
    PermissionDenied { tool: String, required: Vec<Permission> },
    #[error("Tool execution failed: {0}")]
    ExecutionFailed(String),
}
