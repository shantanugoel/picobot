use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::kernel::permissions::{CapabilitySet, Permission};
use crate::scheduler::service::SchedulerService;

#[derive(Debug, Default, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct ToolContext {
    pub capabilities: Arc<CapabilitySet>,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub channel_id: Option<String>,
    pub working_dir: std::path::PathBuf,
    pub jail_root: Option<std::path::PathBuf>,
    pub scheduler: Option<Arc<SchedulerService>>,
    pub scheduled_job: bool,
    pub timezone_offset: String,
    pub timezone_name: String,
}

#[derive(Debug, Clone)]
pub struct ToolError {
    message: String,
    required: Option<Vec<Permission>>,
}

impl ToolError {
    pub fn new(message: String) -> Self {
        Self {
            message,
            required: None,
        }
    }

    pub fn permission_denied(message: String, required: Vec<Permission>) -> Self {
        Self {
            message,
            required: Some(required),
        }
    }

    pub fn required_permissions(&self) -> Option<&[Permission]> {
        self.required.as_deref()
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ToolError {}

pub type ToolOutput = Value;

#[async_trait]
pub trait ToolExecutor: Send + Sync {
    fn spec(&self) -> &ToolSpec;
    fn required_permissions(
        &self,
        ctx: &ToolContext,
        input: &Value,
    ) -> Result<Vec<Permission>, ToolError>;
    async fn execute(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError>;
}
