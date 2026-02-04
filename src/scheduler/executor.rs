use std::sync::Arc;

use dashmap::DashMap;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::config::SchedulerConfig;
use crate::kernel::agent::Kernel;
use crate::kernel::agent_loop::{ConversationState, run_agent_loop_with_limit};
use crate::models::router::ModelRegistry;
use crate::scheduler::job::{ExecutionStatus, JobExecution, ScheduleType, ScheduledJob};
use crate::scheduler::service::next_cron_occurrence;
use crate::scheduler::store::ScheduleStore;

#[derive(Clone)]
pub struct JobExecutor {
    kernel: Arc<Kernel>,
    models: Arc<ModelRegistry>,
    store: ScheduleStore,
    config: SchedulerConfig,
    running: Arc<DashMap<String, CancellationToken>>,
    notifications:
        Arc<tokio::sync::RwLock<Option<crate::notifications::service::NotificationService>>>,
}

impl JobExecutor {
    pub fn new(
        kernel: Arc<Kernel>,
        models: Arc<ModelRegistry>,
        store: ScheduleStore,
        config: SchedulerConfig,
    ) -> Self {
        Self {
            kernel,
            models,
            store,
            config,
            running: Arc::new(DashMap::new()),
            notifications: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }

    pub async fn set_notifications(
        &self,
        service: Option<crate::notifications::service::NotificationService>,
    ) {
        let mut guard = self.notifications.write().await;
        *guard = service;
    }

    pub fn cancel_job(&self, job_id: &str) -> bool {
        if let Some(entry) = self.running.get(job_id) {
            entry.cancel();
            true
        } else {
            false
        }
    }

    pub async fn execute(&self, mut job: ScheduledJob) {
        let execution_id = uuid::Uuid::new_v4().to_string();
        let job_id = job.id.clone();
        let user_id = job.user_id.clone();
        let started_at = chrono::Utc::now();
        let mut execution = JobExecution {
            id: execution_id,
            job_id: job.id.clone(),
            started_at,
            completed_at: None,
            status: ExecutionStatus::Running,
            result_summary: None,
            error: None,
            execution_time_ms: None,
        };
        let _ = self.store.insert_execution(&execution);

        let token = CancellationToken::new();
        self.running.insert(job.id.clone(), token.clone());

        let timeout = Duration::from_secs(self.config.job_timeout_secs());
        println!("scheduler: executing job_id={job_id} user_id={user_id}");
        let outcome = tokio::select! {
            _ = token.cancelled() => ExecutionOutcome::Cancelled,
            result = tokio::time::timeout(timeout, self.run_job(&job)) => {
                match result {
                    Ok(value) => value,
                    Err(_) => ExecutionOutcome::Timeout,
                }
            }
        };

        self.running.remove(&job.id);

        let finished_at = chrono::Utc::now();
        execution.completed_at = Some(finished_at);
        execution.execution_time_ms = Some((finished_at - started_at).num_milliseconds());

        let completion_message = match &outcome {
            ExecutionOutcome::Completed { response } => response.clone(),
            ExecutionOutcome::Failed { error } => Some(format!("Job failed: {error}")),
            ExecutionOutcome::Timeout => Some("Job timed out".to_string()),
            ExecutionOutcome::Cancelled => Some("Job cancelled".to_string()),
        };

        match outcome {
            ExecutionOutcome::Completed { response } => {
                execution.status = ExecutionStatus::Completed;
                execution.result_summary = response.map(|value| truncate(&value, 512));
                job.execution_count = job.execution_count.saturating_add(1);
                job.last_run_at = Some(finished_at);
                job.consecutive_failures = 0;
                job.last_error = None;
                job.backoff_until = None;
                job = apply_next_run(job, finished_at, &self.config);
                if should_disable(&job) {
                    job.enabled = false;
                }
            }
            ExecutionOutcome::Failed { error } => {
                execution.status = ExecutionStatus::Failed;
                execution.error = Some(error.clone());
                job.consecutive_failures = job.consecutive_failures.saturating_add(1);
                job.last_error = Some(error);
                job.backoff_until = Some(
                    finished_at
                        + chrono::Duration::seconds(calculate_backoff_secs(
                            job.consecutive_failures,
                            &self.config,
                        ) as i64),
                );
            }
            ExecutionOutcome::Timeout => {
                execution.status = ExecutionStatus::Timeout;
                execution.error = Some("job timed out".to_string());
                job.consecutive_failures = job.consecutive_failures.saturating_add(1);
                job.last_error = Some("job timed out".to_string());
                job.backoff_until = Some(
                    finished_at
                        + chrono::Duration::seconds(calculate_backoff_secs(
                            job.consecutive_failures,
                            &self.config,
                        ) as i64),
                );
            }
            ExecutionOutcome::Cancelled => {
                execution.status = ExecutionStatus::Cancelled;
                execution.error = Some("job cancelled".to_string());
            }
        }

        println!(
            "scheduler: finished job_id={job_id} user_id={user_id} status={:?}",
            execution.status
        );

        job.claimed_at = None;
        job.claim_id = None;
        job.claim_expires_at = None;
        job.updated_at = finished_at;

        let _ = self.store.update_execution(&execution);
        let _ = self.store.update_job(&job);

        if let Some(channel_id) = job.channel_id.clone() {
            let notification_text =
                completion_message.unwrap_or_else(|| "Job completed".to_string());
            self.enqueue_notification(&job.user_id, &channel_id, notification_text)
                .await;
        }
    }

    async fn run_job(&self, job: &ScheduledJob) -> ExecutionOutcome {
        let mut state = ConversationState::new();
        let scoped_kernel = {
            let mut scoped = self
                .kernel
                .clone_with_context(Some(job.user_id.clone()), job.session_id.clone());
            scoped.set_capabilities(job.capabilities.clone());
            scoped
        };
        let model = self.models.default_model_arc();

        let result = run_agent_loop_with_limit(
            &scoped_kernel,
            model.as_ref(),
            &mut state,
            job.task_prompt.clone(),
            8,
        )
        .await;

        match result {
            Ok(text) => ExecutionOutcome::Completed {
                response: Some(text),
            },
            Err(err) => ExecutionOutcome::Failed {
                error: err.to_string(),
            },
        }
    }

    async fn enqueue_notification(&self, user_id: &str, channel_id: &str, message: String) {
        let service = self.notifications.read().await.clone();
        let Some(service) = service else {
            return;
        };
        let request = crate::notifications::channel::NotificationRequest {
            user_id: user_id.to_string(),
            channel_id: channel_id.to_string(),
            message,
        };
        let _ = service.enqueue(request).await;
    }
}

#[derive(Debug)]
enum ExecutionOutcome {
    Completed { response: Option<String> },
    Failed { error: String },
    Timeout,
    Cancelled,
}

fn calculate_backoff_secs(failures: u32, config: &SchedulerConfig) -> u64 {
    let base = config.tick_interval_secs().max(1);
    let pow = 2u64.saturating_pow(failures.min(16));
    (base.saturating_mul(pow)).min(config.max_backoff_secs())
}

fn apply_next_run(
    mut job: ScheduledJob,
    now: chrono::DateTime<chrono::Utc>,
    config: &SchedulerConfig,
) -> ScheduledJob {
    match job.schedule_type {
        ScheduleType::Interval => {
            if let Some(secs) = job.schedule_interval_seconds() {
                job.next_run_at = now + chrono::Duration::seconds(secs as i64);
            } else {
                job.enabled = false;
                job.last_error = Some("invalid interval schedule".to_string());
            }
        }
        ScheduleType::Once => {
            job.enabled = false;
            job.next_run_at = now + chrono::Duration::seconds(config.tick_interval_secs() as i64);
        }
        ScheduleType::Cron => match next_cron_occurrence(&job.schedule_expr, now) {
            Ok(next) => {
                job.next_run_at = next;
            }
            Err(err) => {
                job.enabled = false;
                job.last_error = Some(err.to_string());
            }
        },
    }
    job
}

fn should_disable(job: &ScheduledJob) -> bool {
    job.max_executions
        .map(|max| job.execution_count >= max)
        .unwrap_or(false)
}

fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        value.to_string()
    } else {
        let mut truncated = value[..max].to_string();
        truncated.push_str("...");
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_next_run, calculate_backoff_secs};
    use crate::config::SchedulerConfig;
    use crate::scheduler::job::{ScheduleType, ScheduledJob};

    fn sample_job(schedule_type: ScheduleType, expr: &str) -> ScheduledJob {
        ScheduledJob {
            id: "job-1".to_string(),
            name: "job".to_string(),
            schedule_type,
            schedule_expr: expr.to_string(),
            task_prompt: "ping".to_string(),
            session_id: None,
            user_id: "user".to_string(),
            channel_id: None,
            capabilities: crate::kernel::permissions::CapabilitySet::empty(),
            creator: crate::scheduler::job::Principal {
                principal_type: crate::scheduler::job::PrincipalType::User,
                id: "user".to_string(),
            },
            enabled: true,
            max_executions: None,
            created_by_system: false,
            execution_count: 0,
            claimed_at: None,
            claim_id: None,
            claim_expires_at: None,
            last_run_at: None,
            next_run_at: chrono::Utc::now(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            consecutive_failures: 0,
            last_error: None,
            backoff_until: None,
            metadata: None,
        }
    }

    #[test]
    fn backoff_is_bounded() {
        let config = SchedulerConfig {
            max_backoff_secs: Some(30),
            tick_interval_secs: Some(2),
            ..Default::default()
        };
        let backoff = calculate_backoff_secs(10, &config);
        assert!(backoff <= 30);
    }

    #[test]
    fn apply_next_run_disables_once() {
        let config = SchedulerConfig::default();
        let job = sample_job(ScheduleType::Once, "now");
        let updated = apply_next_run(job, chrono::Utc::now(), &config);
        assert!(!updated.enabled);
    }

    #[test]
    fn apply_next_run_updates_interval() {
        let config = SchedulerConfig::default();
        let job = sample_job(ScheduleType::Interval, "5");
        let now = chrono::Utc::now();
        let updated = apply_next_run(job, now, &config);
        assert!(updated.next_run_at > now);
    }
}
