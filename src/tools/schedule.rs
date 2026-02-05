use async_trait::async_trait;
use serde_json::{Value, json};

use crate::kernel::permissions::Permission;
use crate::tools::traits::{ToolContext, ToolError, ToolExecutor, ToolOutput, ToolSpec};

#[derive(Debug, Default)]
pub struct ScheduleTool {
    spec: ToolSpec,
}

impl ScheduleTool {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "schedule".to_string(),
                description: "Create, list, or cancel scheduled jobs. Required: action."
                    .to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["action"],
                    "properties": {
                        "action": { "type": "string", "enum": ["create", "list", "cancel"] },
                        "name": { "type": "string" },
                        "schedule_type": { "type": "string", "enum": ["interval", "once", "cron"] },
                        "schedule_expr": { "type": "string" },
                        "task_prompt": { "type": "string" },
                        "session_id": { "type": "string" },
                        "user_id": { "type": "string" },
                        "channel_id": { "type": "string" },
                        "enabled": { "type": "boolean" },
                        "max_executions": { "type": "integer", "minimum": 1 },
                        "metadata": { "type": "object" },
                        "capabilities": { "type": "array", "items": { "type": "string" } },
                        "job_id": { "type": "string" }
                    },
                    "additionalProperties": false
                }),
            },
        }
    }
}

#[async_trait]
impl ToolExecutor for ScheduleTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn required_permissions(
        &self,
        _ctx: &ToolContext,
        input: &Value,
    ) -> Result<Vec<Permission>, ToolError> {
        let action = input
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("missing action".to_string()))?;
        let action = match action {
            "create" | "list" | "cancel" => action,
            _ => return Err(ToolError::new("invalid action".to_string())),
        };
        Ok(vec![Permission::Schedule {
            action: action.to_string(),
        }])
    }

    async fn execute(&self, _ctx: &ToolContext, _input: Value) -> Result<ToolOutput, ToolError> {
        Err(ToolError::new(
            "scheduler not configured in this build".to_string(),
        ))
    }
}
