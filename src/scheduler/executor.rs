use std::sync::Arc;

use dashmap::DashMap;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::config::SchedulerConfig;
use crate::kernel::core::Kernel;
use crate::providers::factory::{ModelRouter, ProviderAgentBuilder};
use crate::scheduler::job::{ExecutionStatus, JobExecution, ScheduledJob};
use crate::scheduler::service::next_cron_occurrence;
use crate::scheduler::store::ScheduleStore;

#[derive(Clone)]
pub struct JobExecutor {
    kernel: Arc<Kernel>,
    store: ScheduleStore,
    config: SchedulerConfig,
    running: Arc<DashMap<String, CancellationToken>>,
    agent_builder: ProviderAgentBuilder,
    router: Option<ModelRouter>,
    fallback_config: crate::config::Config,
}

impl JobExecutor {
    pub fn new(
        kernel: Arc<Kernel>,
        store: ScheduleStore,
        config: SchedulerConfig,
        agent_builder: ProviderAgentBuilder,
        router: Option<ModelRouter>,
        fallback_config: crate::config::Config,
    ) -> Self {
        Self {
            kernel,
            store,
            config,
            running: Arc::new(DashMap::new()),
            agent_builder,
            router,
            fallback_config,
        }
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
        let _job_id = job.id.clone();
        let _user_id = job.user_id.clone();
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

        job.claimed_at = None;
        job.claim_id = None;
        job.claim_expires_at = None;
        job.updated_at = finished_at;

        let _ = self.store.update_execution(&execution);
        let _ = self.store.update_job(&job);

        if let Some(_channel_id) = job.channel_id.clone() {
            let _notification_text =
                completion_message.unwrap_or_else(|| "Job completed".to_string());
        }
    }

    async fn run_job(&self, job: &ScheduledJob) -> ExecutionOutcome {
        let scoped_kernel = self
            .kernel
            .clone_with_context(Some(job.user_id.clone()), job.session_id.clone())
            .with_capabilities(job.capabilities.clone())
            .with_scheduled_job_mode(true)
            .with_channel_id(job.channel_id.clone());
        let channel_id = job.channel_id.clone().unwrap_or_else(|| "scheduler".to_string());
        let base_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let profile = crate::channels::permissions::channel_profile(
            &self.fallback_config.channels(),
            &channel_id,
            &base_dir,
        );
        let scoped_kernel = scoped_kernel.with_prompt_profile(profile);

        let agent = if let Some(router) = self.router.as_ref()
            && !router.is_empty()
        {
            router
                .build_default(
                    &self.fallback_config,
                    scoped_kernel.tool_registry(),
                    Arc::new(scoped_kernel.clone()),
                    8,
                )
                .unwrap_or_else(|_| {
                    self.agent_builder.clone().build_with_env(
                        scoped_kernel.tool_registry(),
                        Arc::new(scoped_kernel.clone()),
                        8,
                        |key| std::env::var(key).ok(),
                    )
                })
        } else {
            self.agent_builder.clone().build_with_env(
                scoped_kernel.tool_registry(),
                Arc::new(scoped_kernel.clone()),
                8,
                |key| std::env::var(key).ok(),
            )
        };
        let response = agent.prompt_with_turns(job.task_prompt.clone(), 8).await;

        match response {
            Ok(text) => ExecutionOutcome::Completed {
                response: Some(text),
            },
            Err(err) => ExecutionOutcome::Failed {
                error: err.to_string(),
            },
        }
    }
}

#[derive(Debug)]
enum ExecutionOutcome {
    Completed { response: Option<String> },
    Failed { error: String },
    Timeout,
    Cancelled,
}

fn apply_next_run(
    mut job: ScheduledJob,
    now: chrono::DateTime<chrono::Utc>,
    _config: &SchedulerConfig,
) -> ScheduledJob {
    job.next_run_at = match job.schedule_type {
        crate::scheduler::job::ScheduleType::Interval => {
            let secs = job.schedule_interval_seconds().unwrap_or(60) as i64;
            now + chrono::Duration::seconds(secs)
        }
        crate::scheduler::job::ScheduleType::Once => now,
        crate::scheduler::job::ScheduleType::Cron => next_cron_occurrence(&job.schedule_expr, now)
            .unwrap_or(now + chrono::Duration::seconds(60)),
    };
    job.backoff_until = None;
    job
}

fn should_disable(job: &ScheduledJob) -> bool {
    if let Some(max) = job.max_executions {
        job.execution_count >= max
    } else {
        false
    }
}

fn calculate_backoff_secs(failures: u32, config: &SchedulerConfig) -> u64 {
    let base = 2u64.saturating_pow(failures.min(10));
    let max = config.max_backoff_secs();
    base.min(max)
}

fn truncate(value: &str, max_len: usize) -> String {
    if value.len() <= max_len {
        return value.to_string();
    }
    let mut out = value.chars().take(max_len).collect::<String>();
    out.push_str("...");
    out
}
