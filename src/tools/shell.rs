use async_trait::async_trait;
use serde_json::{Value, json};

use crate::kernel::permissions::Permission;
use crate::tools::path_utils::resolve_path;
use crate::tools::shell_runner::{ExecutionLimits, HostRunner, ShellRunner};
use crate::tools::shell_policy::{ShellPolicy, ShellRisk};
use crate::tools::traits::{
    PreExecutionDecision,
    PreExecutionPolicy,
    ToolContext,
    ToolError,
    ToolExecutor,
    ToolOutput,
    ToolSpec,
};

#[derive(Debug)]
pub struct ShellTool {
    spec: ToolSpec,
    policy: ShellPolicy,
    runner: std::sync::Arc<dyn ShellRunner>,
    limits: ExecutionLimits,
}

impl ShellTool {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "shell".to_string(),
                description: "Execute a pre-approved shell command. command must be the binary name only (no shell expressions). args is an array of arguments. Only allowlisted commands will succeed. Optional: working_dir."
                    .to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["command"],
                    "properties": {
                        "command": { "type": "string", "minLength": 1 },
                        "args": { "type": "array", "items": { "type": "string" }, "maxItems": 100 },
                        "working_dir": { "type": "string" }
                    },
                    "additionalProperties": false
                }),
            },
            policy: ShellPolicy::default(),
            runner: std::sync::Arc::new(HostRunner),
            limits: ExecutionLimits::default(),
        }
    }

    pub fn with_policy(policy: ShellPolicy) -> Self {
        let mut tool = Self::new();
        tool.policy = policy;
        tool
    }

    pub fn with_runner(mut self, runner: std::sync::Arc<dyn ShellRunner>) -> Self {
        self.runner = runner;
        self
    }

    pub fn with_limits(mut self, limits: ExecutionLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn with_limits_for_timeout(mut self, max_timeout: std::time::Duration) -> Self {
        if self.limits.timeout < max_timeout {
            self.limits.timeout = max_timeout;
        }
        self
    }
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for ShellTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn required_permissions(
        &self,
        _ctx: &ToolContext,
        input: &Value,
    ) -> Result<Vec<Permission>, ToolError> {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("missing command".to_string()))?;
        Ok(vec![Permission::ShellExec {
            allowed_commands: Some(vec![command.to_string()]),
        }])
    }

    fn pre_execution_policy(
        &self,
        _ctx: &ToolContext,
        input: &Value,
    ) -> Result<Option<PreExecutionPolicy>, ToolError> {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("missing command".to_string()))?;
        let args = input
            .get("args")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let args = args
            .iter()
            .filter_map(|value| value.as_str().map(|arg| arg.to_string()))
            .collect::<Vec<_>>();
        let result = self.policy.classify(command, &args);
        let decision = match result.risk {
            ShellRisk::Safe => PreExecutionDecision::Allow,
            ShellRisk::Risky => PreExecutionDecision::RequireApproval,
            ShellRisk::Deny => PreExecutionDecision::Deny,
        };
        Ok(Some(PreExecutionPolicy {
            decision,
            reason: Some(result.reason),
            policy_key: Some(result.policy_key),
        }))
    }

    async fn execute(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("missing command".to_string()))?;
        let args = input
            .get("args")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let args = args
            .iter()
            .filter_map(|value| value.as_str().map(|arg| arg.to_string()))
            .collect::<Vec<_>>();
        let working_dir = input.get("working_dir").and_then(Value::as_str);
        let effective_dir = if let Some(working_dir) = working_dir {
            resolve_path(&ctx.working_dir, ctx.jail_root.as_deref(), working_dir)?.canonical
        } else {
            ctx.working_dir.clone()
        };

        let output = self
            .runner
            .run(command, &args, &effective_dir, &self.limits)
            .await?;
        if output.timed_out {
            return Err(ToolError::timeout("shell command timed out".to_string()));
        }
        Ok(json!({
            "status": if output.exit_code == Some(0) { "ok" } else { "error" },
            "exit_code": output.exit_code,
            "stdout": output.stdout,
            "stderr": output.stderr,
            "truncated": output.truncated
        }))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::ShellTool;
    use crate::tools::shell_policy::ShellPolicy;
    use crate::kernel::permissions::{CapabilitySet, Permission};
    use crate::tools::traits::{
        ExecutionMode,
        PreExecutionDecision,
        ToolContext,
        ToolExecutor,
    };

    #[test]
    fn required_permissions_wrap_command() {
        let tool = ShellTool::new();
        let ctx = ToolContext {
            working_dir: std::path::PathBuf::from("/"),
            capabilities: std::sync::Arc::new(CapabilitySet::empty()),
            user_id: None,
            session_id: None,
            channel_id: None,
            jail_root: None,
            scheduler: None,
            notifications: None,
            notify_tool_used: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            execution_mode: ExecutionMode::User,
            timezone_offset: "+00:00".to_string(),
            timezone_name: "UTC".to_string(),
            max_response_bytes: None,
            max_response_chars: None,
        };
        let required = tool
            .required_permissions(&ctx, &json!({"command": "ls"}))
            .unwrap();
        assert!(matches!(
            required[0],
            Permission::ShellExec {
                allowed_commands: Some(_)
            }
        ));
    }

    #[tokio::test]
    async fn working_dir_denied_outside_jail_root() {
        let tool = ShellTool::new();
        let jail_root = std::env::temp_dir().join(format!("picobot-jail-{}", uuid::Uuid::new_v4()));
        let outside =
            std::env::temp_dir().join(format!("picobot-outside-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&jail_root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        let ctx = ToolContext {
            working_dir: jail_root.clone(),
            capabilities: std::sync::Arc::new(CapabilitySet::empty()),
            user_id: None,
            session_id: None,
            channel_id: None,
            jail_root: Some(jail_root.clone()),
            scheduler: None,
            notifications: None,
            notify_tool_used: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            execution_mode: ExecutionMode::User,
            timezone_offset: "+00:00".to_string(),
            timezone_name: "UTC".to_string(),
            max_response_bytes: None,
            max_response_chars: None,
        };

        let result = tool
            .execute(
                &ctx,
                json!({
                    "command": "echo",
                    "args": ["hi"],
                    "working_dir": outside.to_string_lossy()
                }),
            )
            .await;
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&jail_root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn pre_execution_policy_requires_approval_for_risky() {
        let policy = ShellPolicy::default();
        let tool = ShellTool::with_policy(policy);
        let ctx = ToolContext {
            working_dir: std::path::PathBuf::from("/"),
            capabilities: std::sync::Arc::new(CapabilitySet::empty()),
            user_id: None,
            session_id: None,
            channel_id: None,
            jail_root: None,
            scheduler: None,
            notifications: None,
            notify_tool_used: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            execution_mode: ExecutionMode::User,
            timezone_offset: "+00:00".to_string(),
            timezone_name: "UTC".to_string(),
            max_response_bytes: None,
            max_response_chars: None,
        };
        let decision = tool
            .pre_execution_policy(&ctx, &json!({"command": "rm", "args": ["/tmp"]}))
            .unwrap();
        assert_eq!(
            decision.map(|policy| policy.decision),
            Some(PreExecutionDecision::RequireApproval)
        );
    }

    #[test]
    fn pre_execution_policy_denies_for_blocked_patterns() {
        let policy = ShellPolicy::default();
        let tool = ShellTool::with_policy(policy);
        let ctx = ToolContext {
            working_dir: std::path::PathBuf::from("/"),
            capabilities: std::sync::Arc::new(CapabilitySet::empty()),
            user_id: None,
            session_id: None,
            channel_id: None,
            jail_root: None,
            scheduler: None,
            notifications: None,
            notify_tool_used: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            execution_mode: ExecutionMode::User,
            timezone_offset: "+00:00".to_string(),
            timezone_name: "UTC".to_string(),
            max_response_bytes: None,
            max_response_chars: None,
        };
        let decision = tool
            .pre_execution_policy(&ctx, &json!({"command": "sh", "args": ["-c", "echo hi"]}))
            .unwrap();
        assert_eq!(decision.map(|policy| policy.decision), Some(PreExecutionDecision::Deny));
    }
}
