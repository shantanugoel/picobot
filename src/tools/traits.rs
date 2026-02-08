use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use async_trait::async_trait;
use serde_json::Value;

use crate::kernel::permissions::{CapabilitySet, Permission};
use crate::notifications::service::NotificationService;
use crate::scheduler::service::SchedulerService;

#[derive(Debug, Default, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExecutionMode {
    #[default]
    User,
    ScheduledJob,
    System,
    Admin,
}

impl ExecutionMode {
    pub fn allows_identity_override(self) -> bool {
        matches!(self, ExecutionMode::System | ExecutionMode::Admin)
    }

    pub fn is_scheduled_job(self) -> bool {
        matches!(self, ExecutionMode::ScheduledJob)
    }
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
    pub notifications: Option<Arc<NotificationService>>,
    pub notify_tool_used: Arc<AtomicBool>,
    pub execution_mode: ExecutionMode,
    pub timezone_offset: String,
    pub timezone_name: String,
    pub max_response_bytes: Option<u64>,
    pub max_response_chars: Option<usize>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreExecutionDecision {
    Allow,
    RequireApproval,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreExecutionPolicy {
    pub decision: PreExecutionDecision,
    pub reason: Option<String>,
    pub policy_key: Option<String>,
}

#[async_trait]
pub trait ToolExecutor: Send + Sync {
    fn spec(&self) -> &ToolSpec;
    fn required_permissions(
        &self,
        ctx: &ToolContext,
        input: &Value,
    ) -> Result<Vec<Permission>, ToolError>;
    fn pre_execution_policy(
        &self,
        _ctx: &ToolContext,
        _input: &Value,
    ) -> Result<Option<PreExecutionPolicy>, ToolError> {
        Ok(None)
    }
    async fn execute(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError>;
}
