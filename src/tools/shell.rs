use std::process::Stdio;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::kernel::permissions::Permission;
use crate::tools::traits::{ToolContext, ToolError, ToolExecutor, ToolOutput, ToolSpec};

#[derive(Debug, Default)]
pub struct ShellTool {
    spec: ToolSpec,
}

impl ShellTool {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "shell".to_string(),
                description: "Execute an allowlisted shell command. Required: command. Optional: args, working_dir."
                    .to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["command"],
                    "properties": {
                        "command": { "type": "string" },
                        "args": { "type": "array", "items": { "type": "string" } },
                        "working_dir": { "type": "string" }
                    },
                    "additionalProperties": false
                }),
            },
        }
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
            .map_err(|err| ToolError::new(err.to_string()))?;

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
    use serde_json::json;

    use super::ShellTool;
    use crate::kernel::permissions::{CapabilitySet, Permission};
    use crate::tools::traits::{ToolContext, ToolExecutor};

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
            scheduled_job: false,
            timezone_offset: "+00:00".to_string(),
            timezone_name: "UTC".to_string(),
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
