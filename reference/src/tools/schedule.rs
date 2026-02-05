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
        "Create, list, or cancel scheduled jobs. Required for create: action=create, name, schedule_type, schedule_expr, task_prompt. Required for cancel: action=cancel, job_id. interval uses seconds. cron uses 5 or 6 fields (5 implies leading seconds=0), optional timezone prefix: 'America/New_York|0 30 9 * * *'. Prefer once for one-off reminders with local ISO timestamps. When executing a scheduled job, perform the task prompt only; do not create new schedules unless explicitly requested."
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
        if ctx.scheduled_job {
            return Err(ToolError::ExecutionFailed(
                "schedule tool is disabled during scheduled job execution; use notify for reminders instead"
                    .to_string(),
            ));
        }
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
    let mut schedule_type = parse_schedule_type(schedule_type)?;
    let schedule_expr = input
        .get("schedule_expr")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidInput("missing schedule_expr".to_string()))?;
    let mut schedule_expr = schedule_expr.to_string();
    if matches!(schedule_type, ScheduleType::Cron) {
        schedule_expr = normalize_cron_expr(&schedule_expr)?;
    }
    if matches!(schedule_type, ScheduleType::Once) {
        schedule_expr = normalize_once_expr(&schedule_expr, ctx.timezone_offset.as_str())?;
    }
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
    let mut max_executions = input
        .get("max_executions")
        .and_then(Value::as_u64)
        .map(|value| value as u32);
    if let Some(secs) = parse_relative_duration(&schedule_expr) {
        match schedule_type {
            ScheduleType::Once => {
                schedule_type = ScheduleType::Interval;
                schedule_expr = secs.to_string();
                if max_executions.is_none() {
                    max_executions = Some(1);
                }
            }
            ScheduleType::Interval => {
                schedule_expr = secs.to_string();
            }
            ScheduleType::Cron => {}
        }
    }
    if let Some(duplicate) = find_duplicate_job(
        scheduler,
        &user_id,
        &schedule_type,
        &schedule_expr,
        task_prompt,
        channel_id.as_deref(),
    )? {
        return Ok(json!({
            "status": "existing",
            "job_id": duplicate.id,
            "next_run_at": duplicate.next_run_at,
        }));
    }
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
        schedule_expr,
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

fn parse_relative_duration(value: &str) -> Option<u64> {
    let trimmed = value.trim().to_ascii_lowercase();
    let trimmed = trimmed.strip_prefix("in ").unwrap_or(&trimmed);
    let mut parts = trimmed.split_whitespace();
    let amount = parts.next()?.parse::<u64>().ok()?;
    let unit = parts.next()?;
    match unit {
        "sec" | "secs" | "second" | "seconds" => Some(amount),
        "min" | "mins" | "minute" | "minutes" => Some(amount.saturating_mul(60)),
        "hour" | "hours" => Some(amount.saturating_mul(60 * 60)),
        "day" | "days" => Some(amount.saturating_mul(60 * 60 * 24)),
        _ => None,
    }
}

fn normalize_cron_expr(value: &str) -> Result<String, ToolError> {
    let trimmed = value.trim();
    let (tz, raw) = if let Some((prefix, rest)) = trimmed.split_once('|') {
        let tz = prefix.trim();
        if tz.is_empty() {
            return Err(ToolError::InvalidInput("cron timezone missing".to_string()));
        }
        (Some(tz), rest.trim())
    } else {
        (None, trimmed)
    };
    let field_count = raw.split_whitespace().count();
    let normalized = match field_count {
        5 => format!("0 {raw}"),
        6 | 7 => raw.to_string(),
        _ => {
            return Err(ToolError::InvalidInput(
                "cron expression must have 5 or 6 fields".to_string(),
            ));
        }
    };
    Ok(match tz {
        Some(tz) => format!("{tz}|{normalized}"),
        None => normalized,
    })
}

fn normalize_once_expr(value: &str, tz_offset: &str) -> Result<String, ToolError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ToolError::InvalidInput(
            "once schedule_expr must not be empty".to_string(),
        ));
    }
    if trimmed.contains(' ') && trimmed.contains(':') && !trimmed.contains('T') {
        let replaced = trimmed.replace(' ', "T");
        return Ok(ensure_offset(replaced, tz_offset));
    }
    if trimmed.contains('T') && (trimmed.ends_with('Z') || has_offset_suffix(trimmed)) {
        return Ok(trimmed.to_string());
    }
    if trimmed.contains('T') && !has_offset_suffix(trimmed) {
        return Ok(format!("{trimmed}{tz_offset}"));
    }
    if trimmed.len() == 5 && trimmed.chars().nth(2) == Some(':') {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        return Ok(format!("{today}T{trimmed}:00{tz_offset}"));
    }
    Ok(ensure_offset(trimmed.to_string(), tz_offset))
}

fn ensure_offset(value: String, tz_offset: &str) -> String {
    if value.ends_with('Z') || has_offset_suffix(&value) {
        return value;
    }
    format!("{value}{tz_offset}")
}

fn has_offset_suffix(value: &str) -> bool {
    if value.len() < 6 {
        return false;
    }
    let suffix = &value[value.len().saturating_sub(6)..];
    let bytes = suffix.as_bytes();
    matches!(bytes[0], b'+' | b'-')
        && bytes[3] == b':'
        && bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_digit()
        && bytes[4].is_ascii_digit()
        && bytes[5].is_ascii_digit()
}

fn find_duplicate_job(
    scheduler: &SchedulerService,
    user_id: &str,
    schedule_type: &ScheduleType,
    schedule_expr: &str,
    task_prompt: &str,
    channel_id: Option<&str>,
) -> Result<Option<crate::scheduler::job::ScheduledJob>, ToolError> {
    let jobs = scheduler
        .list_jobs_by_user(user_id)
        .map_err(map_scheduler_error)?;
    let now = chrono::Utc::now();
    let window = chrono::Duration::seconds(120);
    let matching = jobs.into_iter().find(|job| {
        job.enabled
            && job.schedule_type == *schedule_type
            && job.schedule_expr == schedule_expr
            && job.task_prompt == task_prompt
            && job.channel_id.as_deref() == channel_id
            && (now - job.created_at) <= window
    });
    Ok(matching)
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
