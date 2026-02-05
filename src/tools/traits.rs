use std::sync::Arc;

use serde_json::Value;

use crate::kernel::permissions::{CapabilitySet, Permission};

#[derive(Debug, Default, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub capabilities: Arc<CapabilitySet>,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub working_dir: std::path::PathBuf,
}

#[derive(Debug, Clone)]
pub struct ToolError {
    message: String,
}

impl ToolError {
    pub fn new(message: String) -> Self {
        Self { message }
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ToolError {}

pub type ToolOutput = Value;

pub trait ToolExecutor: Send + Sync {
    fn spec(&self) -> &ToolSpec;
    fn required_permissions(
        &self,
        ctx: &ToolContext,
        input: &Value,
    ) -> Result<Vec<Permission>, ToolError>;
    fn execute(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError>;
}
