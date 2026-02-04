use async_trait::async_trait;
use serde_json::{Value, json};

use crate::kernel::context::ToolContext;
use crate::kernel::permissions::{CapabilitySet, Permission};
use crate::scheduler::error::SchedulerError;
use crate::scheduler::job::{CreateJobRequest, Principal, PrincipalType, ScheduleType};
use crate::scheduler::service::SchedulerService;
use crate::tools::traits::{Tool, ToolError, ToolOutput};

#[derive(Debug, Clone)]
pub struct ScheduleTool;

#[async_trait]
impl Tool for ScheduleTool {
    fn name(&self) -> &'static str {
        "schedule"
    }

    fn description(&self) -> &'static str {
        "Create, list, or cancel scheduled jobs"
    }

    fn schema(&self) -> Value {
        json!({
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
        })
    }

    fn required_permissions(
        &self,
        _ctx: &ToolContext,
        input: &Value,
    ) -> Result<Vec<Permission>, ToolError> {
        let action = input
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("missing action".to_string()))?;
        let action = match action {
            "create" | "list" | "cancel" => action,
            _ => return Err(ToolError::InvalidInput("invalid action".to_string())),
        };
        Ok(vec![Permission::Schedule {
            action: action.to_string(),
        }])
    }

    async fn execute(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        let scheduler = ctx
            .scheduler()
            .ok_or_else(|| ToolError::ExecutionFailed("scheduler not available".to_string()))?;
        let action = input
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("missing action".to_string()))?;
        match action {
            "create" => create_job(&scheduler, ctx, &input),
            "list" => list_jobs(&scheduler, ctx),
            "cancel" => cancel_job(&scheduler, ctx, &input),
            _ => Err(ToolError::InvalidInput("invalid action".to_string())),
        }
    }
}

fn create_job(
    scheduler: &SchedulerService,
    ctx: &ToolContext,
    input: &Value,
) -> Result<ToolOutput, ToolError> {
    let name = input
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidInput("missing name".to_string()))?;
    let schedule_type = input
        .get("schedule_type")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidInput("missing schedule_type".to_string()))?;
    let schedule_type = parse_schedule_type(schedule_type)?;
    let schedule_expr = input
        .get("schedule_expr")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidInput("missing schedule_expr".to_string()))?;
    let task_prompt = input
        .get("task_prompt")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidInput("missing task_prompt".to_string()))?;
    let session_id = input
        .get("session_id")
        .and_then(Value::as_str)
        .map(|value| value.to_string());
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
        .or_else(|| channel_id_from_session(ctx.session_id.as_deref()));
    let enabled = input
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let max_executions = input
        .get("max_executions")
        .and_then(Value::as_u64)
        .map(|value| value as u32);
    let metadata = input.get("metadata").cloned();
    let requested = input
        .get("capabilities")
        .map(parse_capabilities)
        .transpose()?;
    let capabilities = match requested {
        Some(value) if capabilities_subset(ctx.capabilities.as_ref(), &value) => value,
        _ => ctx.capabilities.as_ref().clone(),
    };
    let request = CreateJobRequest {
        name: name.to_string(),
        schedule_type,
        schedule_expr: schedule_expr.to_string(),
        task_prompt: task_prompt.to_string(),
        session_id,
        user_id: user_id.clone(),
        channel_id,
        capabilities,
        creator: Principal {
            principal_type: PrincipalType::User,
            id: user_id,
        },
        enabled,
        max_executions,
        created_by_system: false,
        metadata,
    };
    scheduler
        .create_job(request)
        .map(|job| {
            json!({
                "status": "created",
                "job_id": job.id,
                "next_run_at": job.next_run_at,
            })
        })
        .map_err(map_scheduler_error)
}

fn channel_id_from_session(session_id: Option<&str>) -> Option<String> {
    let session_id = session_id?;
    session_id
        .split_once(':')
        .map(|(channel, _)| channel.to_string())
}

fn list_jobs(scheduler: &SchedulerService, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
    let user_id = ctx
        .user_id
        .as_ref()
        .ok_or_else(|| ToolError::ExecutionFailed("missing user_id".to_string()))?;
    scheduler
        .list_jobs_by_user(user_id)
        .map(|jobs| {
            let items = jobs
                .into_iter()
                .map(|job| {
                    json!({
                        "id": job.id,
                        "name": job.name,
                        "schedule_type": job.schedule_type,
                        "schedule_expr": job.schedule_expr,
                        "enabled": job.enabled,
                        "execution_count": job.execution_count,
                        "next_run_at": job.next_run_at,
                        "last_run_at": job.last_run_at,
                        "last_error": job.last_error,
                    })
                })
                .collect::<Vec<_>>();
            json!({"schedules": items})
        })
        .map_err(map_scheduler_error)
}

fn cancel_job(
    scheduler: &SchedulerService,
    ctx: &ToolContext,
    input: &Value,
) -> Result<ToolOutput, ToolError> {
    let job_id = input
        .get("job_id")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidInput("missing job_id".to_string()))?;
    let user_id = ctx
        .user_id
        .as_ref()
        .ok_or_else(|| ToolError::ExecutionFailed("missing user_id".to_string()))?;
    let job = scheduler
        .store()
        .get_job(job_id)
        .map_err(map_scheduler_error)?
        .ok_or_else(|| ToolError::ExecutionFailed("job not found".to_string()))?;
    if job.user_id != *user_id {
        return Err(ToolError::ExecutionFailed(
            "job not owned by user".to_string(),
        ));
    }
    let cancelled = scheduler.cancel_job(job_id).map_err(map_scheduler_error)?;
    Ok(json!({"status": "cancelled", "running": cancelled}))
}

fn parse_schedule_type(value: &str) -> Result<ScheduleType, ToolError> {
    match value {
        "interval" => Ok(ScheduleType::Interval),
        "once" => Ok(ScheduleType::Once),
        "cron" => Ok(ScheduleType::Cron),
        _ => Err(ToolError::InvalidInput("invalid schedule_type".to_string())),
    }
}

fn capabilities_subset(parent: &CapabilitySet, child: &CapabilitySet) -> bool {
    child
        .permissions()
        .all(|permission| parent.allows(permission))
}

fn parse_capabilities(value: &Value) -> Result<CapabilitySet, ToolError> {
    let entries = value
        .as_array()
        .ok_or_else(|| ToolError::InvalidInput("capabilities must be an array".to_string()))?;
    let mut permissions = Vec::with_capacity(entries.len());
    for entry in entries {
        let raw = entry
            .as_str()
            .ok_or_else(|| ToolError::InvalidInput("capability must be a string".to_string()))?;
        let permission = raw
            .parse::<Permission>()
            .map_err(|err| ToolError::InvalidInput(err.to_string()))?;
        permissions.push(permission);
    }
    Ok(CapabilitySet::from_permissions(&permissions))
}

fn map_scheduler_error(err: SchedulerError) -> ToolError {
    match err {
        SchedulerError::PermissionDenied(detail) => ToolError::ExecutionFailed(detail),
        SchedulerError::InvalidSchedule(detail) => ToolError::InvalidInput(detail),
        other => ToolError::ExecutionFailed(other.to_string()),
    }
}
