use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

use crate::kernel::permissions::Permission;
use crate::notifications::channel::NotificationRequest;
use crate::tools::traits::{ToolContext, ToolError, ToolExecutor, ToolOutput, ToolSpec};

#[derive(Debug, Default)]
pub struct NotifyTool {
    spec: ToolSpec,
}

impl NotifyTool {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "notify".to_string(),
                description: "Send a notification message to the current user on the current channel. Only requires message. Use for reminders, alerts, and delivering results from scheduled jobs.".to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["message"],
                    "properties": {
                        "message": { "type": "string", "minLength": 1 }
                    },
                    "additionalProperties": false
                }),
            },
        }
    }
}

#[async_trait]
impl ToolExecutor for NotifyTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn required_permissions(
        &self,
        ctx: &ToolContext,
        input: &Value,
    ) -> Result<Vec<Permission>, ToolError> {
        let channel = input
            .get("channel_id")
            .and_then(Value::as_str)
            .map(|value| value.to_string())
            .or_else(|| ctx.channel_id.clone())
            .or_else(|| {
                ctx.session_id.as_deref().and_then(|value| {
                    value
                        .split_once(':')
                        .map(|(channel, _)| channel.to_string())
                })
            })
            .unwrap_or_else(|| "*".to_string());
        Ok(vec![Permission::Notify { channel }])
    }

    async fn execute(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        let message = input
            .get("message")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("missing message".to_string()))?;
        let input_user = input
            .get("user_id")
            .and_then(Value::as_str)
            .map(|value| value.to_string());
        let ctx_user = ctx
            .user_id
            .as_ref()
            .ok_or_else(|| ToolError::new("missing user_id".to_string()))?;
        if !ctx.execution_mode.allows_identity_override()
            && let Some(input_user) = input_user.as_deref()
            && input_user != ctx_user
        {
            tracing::warn!(
                event = "identity_mismatch",
                tool = "notify",
                field = "user_id",
                input = %input_user,
                context = %ctx_user,
                "notify user_id does not match context"
            );
            return Err(ToolError::new("user_id does not match context".to_string()));
        }
        let user_id = input_user.unwrap_or_else(|| ctx_user.to_string());
        let input_channel = input
            .get("channel_id")
            .and_then(Value::as_str)
            .map(|value| value.to_string());
        let ctx_channel = ctx
            .channel_id
            .clone()
            .or_else(|| {
                ctx.session_id.as_deref().and_then(|value| {
                    value
                        .split_once(':')
                        .map(|(channel, _)| channel.to_string())
                })
            })
            .ok_or_else(|| ToolError::new("missing channel_id".to_string()))?;
        if !ctx.execution_mode.allows_identity_override()
            && let Some(input_channel) = input_channel.as_deref()
            && input_channel != ctx_channel.as_str()
        {
            tracing::warn!(
                event = "identity_mismatch",
                tool = "notify",
                field = "channel_id",
                input = %input_channel,
                context = %ctx_channel,
                "notify channel_id does not match context"
            );
            return Err(ToolError::new(
                "channel_id does not match context".to_string(),
            ));
        }
        let channel_id = input_channel
            .or_else(|| ctx.channel_id.clone())
            .or_else(|| {
                ctx.session_id.as_deref().and_then(|value| {
                    value
                        .split_once(':')
                        .map(|(channel, _)| channel.to_string())
                })
            })
            .ok_or_else(|| ToolError::new("missing channel_id".to_string()))?;
        let service = ctx.notifications.as_ref().ok_or_else(|| {
            tracing::warn!(
                event = "notification_restricted",
                reason = "service_unavailable",
                tool = "notify",
                user_id = ?ctx.user_id,
                session_id = ?ctx.session_id,
                channel_id = ?ctx.channel_id,
                "notifications not available"
            );
            ToolError::new("notifications not available".to_string())
        })?;
        let request = NotificationRequest {
            user_id: user_id.clone(),
            channel_id: channel_id.clone(),
            message: message.to_string(),
        };
        let id = service.enqueue(request).await;
        ctx.notify_tool_used.store(true, Ordering::Relaxed);
        Ok(json!({"status": "queued", "id": id}))
    }
}
