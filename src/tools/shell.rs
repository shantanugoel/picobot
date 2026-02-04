use std::process::Stdio;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::kernel::permissions::Permission;
use crate::tools::traits::{Tool, ToolContext, ToolError, ToolOutput};

#[derive(Debug, Default)]
pub struct ShellTool;

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &'static str {
        "shell"
    }

    fn description(&self) -> &'static str {
        "Execute a shell command from the allowlist"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": { "type": "string" },
                "args": { "type": "array", "items": { "type": "string" } },
                "working_dir": { "type": "string" }
            },
            "additionalProperties": false
        })
    }

    fn required_permissions(
        &self,
        _ctx: &ToolContext,
        input: &Value,
    ) -> Result<Vec<Permission>, ToolError> {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("missing command".to_string()))?;
        let permission = Permission::ShellExec {
            allowed_commands: Some(vec![command.to_string()]),
        };
        Ok(vec![permission])
    }

    async fn execute(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("missing command".to_string()))?;
        let args = input
            .get("args")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let working_dir = input.get("working_dir").and_then(Value::as_str);

        let mut cmd = tokio::process::Command::new(command);
        if let Some(working_dir) = working_dir {
            cmd.current_dir(working_dir);
        } else {
            cmd.current_dir(&ctx.working_dir);
        }
        for arg in args {
            if let Some(arg) = arg.as_str() {
                cmd.arg(arg);
            }
        }
        let output = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Ok(json!({
            "status": if output.status.success() { "ok" } else { "error" },
            "exit_code": output.status.code(),
            "stdout": stdout,
            "stderr": stderr
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::ShellTool;
    use crate::kernel::permissions::Permission;
    use crate::tools::traits::Tool;
    use serde_json::json;

    #[test]
    fn required_permissions_wrap_command() {
        let tool = ShellTool;
        let ctx = crate::tools::traits::ToolContext {
            working_dir: std::path::PathBuf::from("/"),
            capabilities: std::sync::Arc::new(crate::kernel::permissions::CapabilitySet::empty()),
            user_id: None,
            session_id: None,
            scheduler: std::sync::Arc::new(std::sync::RwLock::new(None)),
            log_model_requests: false,
            include_tool_messages: true,
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
}
