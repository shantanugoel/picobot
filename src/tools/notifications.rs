use async_trait::async_trait;
use serde_json::{Value, json};

use crate::kernel::context::ToolContext;
use crate::kernel::permissions::Permission;
use crate::tools::traits::{Tool, ToolError, ToolOutput};

#[derive(Debug, Default)]
pub struct NotificationTool;

#[async_trait]
impl Tool for NotificationTool {
    fn name(&self) -> &'static str {
        "notify"
    }

    fn description(&self) -> &'static str {
        "Send a user notification. Required: message. Optional: channel_id, user_id (defaults to current context). Use this for reminders, alerts, and direct user messages (e.g., 'remind', 'tell', 'notify'). Use inside scheduled jobs instead of scheduling new jobs."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["message"],
            "properties": {
                "message": { "type": "string" },
                "channel_id": { "type": "string" },
                "user_id": { "type": "string" }
            },
            "additionalProperties": false
        })
    }

    fn required_permissions(
        &self,
        _ctx: &ToolContext,
        _input: &Value,
    ) -> Result<Vec<Permission>, ToolError> {
        Ok(vec![])
    }

    async fn execute(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        let message = input
            .get("message")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("missing message".to_string()))?;
        let user_id = input
            .get("user_id")
            .and_then(Value::as_str)
            .map(|value| value.to_string())
            .or_else(|| ctx.user_id.clone())
            .ok_or_else(|| ToolError::ExecutionFailed("missing user_id".to_string()))?;
        let channel_id = input
            .get("channel_id")
            .and_then(Value::as_str)
            .map(|value| value.to_string())
            .or_else(|| ctx.channel_id.clone())
            .or_else(|| {
                ctx.session_id
                    .as_deref()
                    .and_then(|value| value.split_once(':').map(|(c, _)| c.to_string()))
            })
            .ok_or_else(|| ToolError::ExecutionFailed("missing channel_id".to_string()))?;
        let service = ctx
            .notifications()
            .ok_or_else(|| ToolError::ExecutionFailed("notifications not available".to_string()))?;
        let request = crate::notifications::channel::NotificationRequest {
            user_id: user_id.clone(),
            channel_id: channel_id.clone(),
            message: message.to_string(),
        };
        let id = service.enqueue(request).await;
        Ok(json!({"status": "queued", "id": id}))
    }
}
