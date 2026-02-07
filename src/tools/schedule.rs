use async_trait::async_trait;
use serde_json::{Value, json};

use crate::kernel::permissions::{CapabilitySet, Permission};
use crate::scheduler::job::{CreateJobRequest, Principal, PrincipalType, ScheduleType};
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
                        "schedule_expr": { "type": "string", "description": "For interval: seconds or relative duration (e.g. '2 minutes'). For once: relative duration (e.g. '2 minutes') or RFC3339 datetime. For cron: cron expression." },
                        "task_prompt": { "type": "string", "description": "User-facing message to send when the job runs." },
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
        Ok(vec![
            Permission::Schedule {
                action: action.to_string(),
            },
            Permission::Schedule {
                action: "*".to_string(),
            },
        ])
    }

    async fn execute(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        if ctx.execution_mode.is_scheduled_job() {
            return Err(ToolError::new(
                "schedule tool is disabled during scheduled job execution; use notify for reminders instead"
                    .to_string(),
            ));
        }
        let scheduler = ctx
            .scheduler
            .as_ref()
            .ok_or_else(|| ToolError::new("scheduler not available".to_string()))?;
        let action = input
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("missing action".to_string()))?;
        match action {
            "create" => create_job(scheduler, ctx, &input),
            "list" => list_jobs(scheduler, ctx),
            "cancel" => cancel_job(scheduler, ctx, &input),
            _ => Err(ToolError::new("invalid action".to_string())),
        }
    }
}

fn create_job(
    scheduler: &crate::scheduler::service::SchedulerService,
    ctx: &ToolContext,
    input: &Value,
) -> Result<ToolOutput, ToolError> {
    let schedule_type = input
        .get("schedule_type")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("missing schedule_type".to_string()))?;
    let mut schedule_type = parse_schedule_type(schedule_type)?;
    let schedule_expr = input
        .get("schedule_expr")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("missing schedule_expr".to_string()))?;
    let mut schedule_expr = schedule_expr.to_string();
    if matches!(schedule_type, ScheduleType::Cron) {
        schedule_expr = normalize_cron_expr(&schedule_expr)?;
    }
    let task_prompt = input
        .get("task_prompt")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("missing task_prompt".to_string()))?;
    let name = input
        .get("name")
        .and_then(Value::as_str)
        .map(|value| value.to_string())
        .unwrap_or_else(|| default_job_name(task_prompt));
    let input_session = input
        .get("session_id")
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    if !ctx.execution_mode.allows_identity_override()
        && let Some(input_session) = input_session.as_deref()
    {
        match ctx.session_id.as_deref() {
            Some(ctx_session) if ctx_session == input_session => {}
            Some(ctx_session) => {
                tracing::warn!(
                    event = "identity_mismatch",
                    tool = "schedule",
                    field = "session_id",
                    input = %input_session,
                    context = %ctx_session,
                    "schedule session_id does not match context"
                );
                return Err(ToolError::new(
                    "session_id does not match context".to_string(),
                ));
            }
            None => {
                tracing::warn!(
                    event = "identity_mismatch",
                    tool = "schedule",
                    field = "session_id",
                    input = %input_session,
                    context = "missing",
                    "schedule session_id missing from context"
                );
                return Err(ToolError::new("missing session_id in context".to_string()));
            }
        }
    }
    let session_id = input_session.or_else(|| ctx.session_id.clone());
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
            tool = "schedule",
            field = "user_id",
            input = %input_user,
            context = %ctx_user,
            "schedule user_id does not match context"
        );
        return Err(ToolError::new("user_id does not match context".to_string()));
    }
    let user_id = input_user.unwrap_or_else(|| ctx_user.to_string());
    let input_channel = input
        .get("channel_id")
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    if !ctx.execution_mode.allows_identity_override()
        && let Some(input_channel) = input_channel.as_deref()
    {
        match ctx.channel_id.as_deref() {
            Some(ctx_channel) if ctx_channel == input_channel => {}
            Some(ctx_channel) => {
                tracing::warn!(
                    event = "identity_mismatch",
                    tool = "schedule",
                    field = "channel_id",
                    input = %input_channel,
                    context = %ctx_channel,
                    "schedule channel_id does not match context"
                );
                return Err(ToolError::new(
                    "channel_id does not match context".to_string(),
                ));
            }
            None => {
                tracing::warn!(
                    event = "identity_mismatch",
                    tool = "schedule",
                    field = "channel_id",
                    input = %input_channel,
                    context = "missing",
                    "schedule channel_id missing from context"
                );
                return Err(ToolError::new("missing channel_id in context".to_string()));
            }
        }
    }
    let channel_id = input_channel
        .or_else(|| ctx.channel_id.clone())
        .or_else(|| session_id.as_deref().and_then(channel_id_from_session));
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
    } else if matches!(schedule_type, ScheduleType::Once) {
        schedule_expr = normalize_once_expr(&schedule_expr, ctx.timezone_offset.as_str())?;
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
        .map_err(|err| ToolError::new(err.to_string()))
}

fn channel_id_from_session(session_id: &str) -> Option<String> {
    session_id
        .split_once(':')
        .map(|(channel, _)| channel.to_string())
}

fn default_job_name(task_prompt: &str) -> String {
    let trimmed = task_prompt.trim();
    if trimmed.is_empty() {
        return "scheduled-job".to_string();
    }
    let mut out = String::new();
    let mut last_dash = false;
    for ch in trimmed.chars() {
        if out.len() >= 40 {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "scheduled-job".to_string()
    } else {
        trimmed.to_string()
    }
}

fn list_jobs(
    scheduler: &crate::scheduler::service::SchedulerService,
    ctx: &ToolContext,
) -> Result<ToolOutput, ToolError> {
    let user_id = ctx
        .user_id
        .as_ref()
        .ok_or_else(|| ToolError::new("missing user_id".to_string()))?;
    let jobs = if let Some(session_id) = ctx.session_id.as_deref() {
        scheduler
            .store()
            .list_jobs_by_user_with_session(user_id, session_id)
    } else {
        scheduler.list_jobs_by_user(user_id)
    };
    jobs.map(|jobs| {
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
    .map_err(|err| ToolError::new(err.to_string()))
}

fn cancel_job(
    scheduler: &crate::scheduler::service::SchedulerService,
    ctx: &ToolContext,
    input: &Value,
) -> Result<ToolOutput, ToolError> {
    let job_id = input
        .get("job_id")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("missing job_id".to_string()))?;
    let user_id = ctx
        .user_id
        .as_ref()
        .ok_or_else(|| ToolError::new("missing user_id".to_string()))?;
    let job = scheduler
        .store()
        .get_job(job_id)
        .map_err(|err| ToolError::new(err.to_string()))?
        .ok_or_else(|| ToolError::new("job not found".to_string()))?;
    if job.user_id != *user_id {
        tracing::warn!(
            event = "permission_denied",
            reason = "identity_mismatch",
            tool = "schedule",
            job_id = %job_id,
            input_user = %user_id,
            owner = %job.user_id,
            "schedule cancel denied: job not owned by user"
        );
        return Err(ToolError::new("job not owned by user".to_string()));
    }
    let cancelled = scheduler
        .cancel_job(job_id)
        .map_err(|err| ToolError::new(err.to_string()))?;
    Ok(json!({"status": "cancelled", "running": cancelled}))
}

fn parse_schedule_type(value: &str) -> Result<ScheduleType, ToolError> {
    match value {
        "interval" => Ok(ScheduleType::Interval),
        "once" => Ok(ScheduleType::Once),
        "cron" => Ok(ScheduleType::Cron),
        _ => Err(ToolError::new("invalid schedule_type".to_string())),
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
        .ok_or_else(|| ToolError::new("capabilities must be an array".to_string()))?;
    let mut parsed = Vec::new();
    for entry in entries {
        let entry = entry
            .as_str()
            .ok_or_else(|| ToolError::new("capabilities must be strings".to_string()))?;
        let permission = entry.parse::<Permission>().map_err(ToolError::new)?;
        parsed.push(permission);
    }
    Ok(CapabilitySet::from_permissions(&parsed))
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
            return Err(ToolError::new("cron timezone missing".to_string()));
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
            return Err(ToolError::new(
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
        return Err(ToolError::new(
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
    scheduler: &crate::scheduler::service::SchedulerService,
    user_id: &str,
    schedule_type: &ScheduleType,
    schedule_expr: &str,
    task_prompt: &str,
    channel_id: Option<&str>,
) -> Result<Option<crate::scheduler::job::ScheduledJob>, ToolError> {
    let jobs = scheduler
        .list_jobs_by_user(user_id)
        .map_err(|err| ToolError::new(err.to_string()))?;
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
