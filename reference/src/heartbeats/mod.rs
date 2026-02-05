use std::sync::Arc;

use crate::config::{HeartbeatPromptConfig, HeartbeatsConfig};
use crate::kernel::permissions::{CapabilitySet, Permission};
use crate::scheduler::job::{CreateJobRequest, Principal, PrincipalType, ScheduleType};
use crate::scheduler::service::SchedulerService;

#[derive(Debug, Clone)]
pub struct HeartbeatRegistrationSummary {
    pub created: usize,
    pub skipped_existing: usize,
    pub skipped_disabled: usize,
}

pub fn register_heartbeats(
    scheduler: &Arc<SchedulerService>,
    config: &HeartbeatsConfig,
) -> HeartbeatRegistrationSummary {
    if !config.enabled() {
        return HeartbeatRegistrationSummary {
            created: 0,
            skipped_existing: 0,
            skipped_disabled: config.prompts.len(),
        };
    }

    let mut summary = HeartbeatRegistrationSummary {
        created: 0,
        skipped_existing: 0,
        skipped_disabled: 0,
    };

    for prompt in &config.prompts {
        match ensure_heartbeat(scheduler, config, prompt) {
            HeartbeatResult::Created => summary.created += 1,
            HeartbeatResult::SkippedExisting => summary.skipped_existing += 1,
            HeartbeatResult::SkippedDisabled => summary.skipped_disabled += 1,
            HeartbeatResult::Error => summary.skipped_disabled += 1,
        }
    }

    summary
}

#[derive(Debug, Clone, Copy)]
enum HeartbeatResult {
    Created,
    SkippedExisting,
    SkippedDisabled,
    Error,
}

fn ensure_heartbeat(
    scheduler: &Arc<SchedulerService>,
    config: &HeartbeatsConfig,
    prompt: &HeartbeatPromptConfig,
) -> HeartbeatResult {
    let name = heartbeat_name(prompt);
    let user_id = "system:heartbeats".to_string();
    let jobs = match scheduler.list_jobs() {
        Ok(jobs) => jobs,
        Err(_) => return HeartbeatResult::Error,
    };
    if jobs
        .iter()
        .any(|job| job.name == name && job.created_by_system)
    {
        return HeartbeatResult::SkippedExisting;
    }

    let (schedule_type, schedule_expr) = match schedule_from_prompt(config, prompt) {
        Some(value) => value,
        None => return HeartbeatResult::SkippedDisabled,
    };

    let request = CreateJobRequest {
        name,
        schedule_type,
        schedule_expr,
        task_prompt: prompt.prompt.clone(),
        session_id: None,
        user_id: user_id.clone(),
        channel_id: None,
        capabilities: heartbeat_capabilities(),
        creator: Principal {
            principal_type: PrincipalType::System,
            id: "heartbeats".to_string(),
        },
        enabled: true,
        max_executions: None,
        created_by_system: true,
        metadata: Some(serde_json::json!({
            "heartbeat": true,
            "name": prompt.name,
        })),
    };

    match scheduler.create_job(request) {
        Ok(_) => HeartbeatResult::Created,
        Err(_) => HeartbeatResult::Error,
    }
}

fn schedule_from_prompt(
    config: &HeartbeatsConfig,
    prompt: &HeartbeatPromptConfig,
) -> Option<(ScheduleType, String)> {
    if let Some(expr) = prompt.cron.as_ref() {
        let timezone = prompt.timezone.as_deref().unwrap_or("UTC");
        return Some((ScheduleType::Cron, format!("{timezone}|{expr}")));
    }
    let interval = prompt
        .interval_secs
        .unwrap_or_else(|| config.default_interval_secs());
    if interval == 0 {
        return None;
    }
    Some((ScheduleType::Interval, interval.to_string()))
}

fn heartbeat_name(prompt: &HeartbeatPromptConfig) -> String {
    format!("heartbeat:{}", prompt.name)
}

fn heartbeat_capabilities() -> CapabilitySet {
    let mut capabilities = CapabilitySet::empty();
    capabilities.insert(Permission::Schedule {
        action: "create".to_string(),
    });
    capabilities
}
