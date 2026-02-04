use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::Semaphore;

use crate::config::SchedulerConfig;
use crate::kernel::permissions::Permission;
use crate::scheduler::error::{SchedulerError, SchedulerResult};
use crate::scheduler::executor::JobExecutor;
use crate::scheduler::job::{CreateJobRequest, ScheduleType, ScheduledJob};
use crate::scheduler::store::ScheduleStore;

#[derive(Clone)]
pub struct SchedulerService {
    store: ScheduleStore,
    executor: JobExecutor,
    config: SchedulerConfig,
    global_semaphore: Arc<Semaphore>,
    per_user_semaphores: Arc<DashMap<String, Arc<Semaphore>>>,
}

impl SchedulerService {
    pub fn new(store: ScheduleStore, executor: JobExecutor, config: SchedulerConfig) -> Self {
        let global_semaphore = Arc::new(Semaphore::new(config.max_concurrent_jobs()));
        Self {
            store,
            executor,
            config,
            global_semaphore,
            per_user_semaphores: Arc::new(DashMap::new()),
        }
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled()
    }

    pub async fn run_loop(&self) {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(
            self.config.tick_interval_secs(),
        ));
        loop {
            interval.tick().await;
            if !self.enabled() {
                continue;
            }
            self.tick().await;
        }
    }

    pub async fn tick(&self) {
        let now = chrono::Utc::now();
        let claim_id = uuid::Uuid::new_v4().to_string();
        let claim_limit = self.config.max_concurrent_jobs().max(1);
        let lease_secs = (self.config.tick_interval_secs() * 2).max(2);
        let jobs = match self
            .store
            .claim_due_jobs(now, claim_limit, &claim_id, lease_secs)
        {
            Ok(jobs) => jobs,
            Err(_) => return,
        };
        for job in jobs {
            if self.global_semaphore.available_permits() == 0 {
                let _ = self.store.release_claim(&job.id, &claim_id);
                continue;
            }
            let user_semaphore = self
                .per_user_semaphores
                .entry(job.user_id.clone())
                .or_insert_with(|| Arc::new(Semaphore::new(self.config.max_concurrent_per_user())))
                .clone();
            if user_semaphore.available_permits() == 0 {
                let _ = self.store.release_claim(&job.id, &claim_id);
                continue;
            }
            let global_permit = match self.global_semaphore.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    let _ = self.store.release_claim(&job.id, &claim_id);
                    continue;
                }
            };
            let user_permit = match user_semaphore.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    drop(global_permit);
                    let _ = self.store.release_claim(&job.id, &claim_id);
                    continue;
                }
            };
            let executor = self.executor.clone();
            tokio::spawn(async move {
                let _global = global_permit;
                let _user = user_permit;
                executor.execute(job).await;
            });
        }
    }

    pub fn create_job(&self, request: CreateJobRequest) -> SchedulerResult<ScheduledJob> {
        if !self.enabled() {
            return Err(SchedulerError::Disabled);
        }
        self.ensure_schedule_permission(&request.capabilities)?;
        self.enforce_quotas(&request.user_id)?;
        let next_run_at = compute_initial_run(&request)?;
        self.store.create_job(request, next_run_at)
    }

    pub fn delete_job_with_cancel(&self, job_id: &str) -> SchedulerResult<()> {
        let _ = self.executor.cancel_job(job_id);
        self.store.delete_job(job_id)
    }

    pub fn list_jobs_by_user(&self, user_id: &str) -> SchedulerResult<Vec<ScheduledJob>> {
        self.store.list_jobs_by_user(user_id)
    }

    pub fn list_jobs(&self) -> SchedulerResult<Vec<ScheduledJob>> {
        self.store.list_jobs()
    }

    pub fn get_job(&self, job_id: &str) -> SchedulerResult<ScheduledJob> {
        self.store
            .get_job(job_id)
            .and_then(|job| job.ok_or(SchedulerError::NotFound))
    }

    pub fn update_job(&self, job: &ScheduledJob) -> SchedulerResult<()> {
        self.store.update_job(job)
    }

    pub fn delete_job(&self, job_id: &str) -> SchedulerResult<()> {
        self.store.delete_job(job_id)
    }

    pub fn cancel_job(&self, job_id: &str) -> SchedulerResult<bool> {
        Ok(self.executor.cancel_job(job_id))
    }

    pub fn list_executions_for_job(
        &self,
        job_id: &str,
        limit: usize,
        offset: usize,
    ) -> SchedulerResult<Vec<crate::scheduler::job::JobExecution>> {
        self.store.list_executions_for_job(job_id, limit, offset)
    }

    pub fn list_all_executions(&self) -> SchedulerResult<Vec<crate::scheduler::job::JobExecution>> {
        self.store.list_all_executions()
    }

    pub fn store(&self) -> &ScheduleStore {
        &self.store
    }

    fn ensure_schedule_permission(
        &self,
        capabilities: &crate::kernel::permissions::CapabilitySet,
    ) -> SchedulerResult<()> {
        let required = Permission::Schedule {
            action: "create".to_string(),
        };
        if capabilities.allows(&required) {
            return Ok(());
        }
        Err(SchedulerError::PermissionDenied(
            "missing schedule:create capability".to_string(),
        ))
    }

    fn enforce_quotas(&self, user_id: &str) -> SchedulerResult<()> {
        let per_user = self.store.count_jobs_for_user(user_id)?;
        if per_user >= self.config.max_jobs_per_user() {
            return Err(SchedulerError::QuotaExceeded(
                "max jobs per user exceeded".to_string(),
            ));
        }
        let window_start = chrono::Utc::now()
            - chrono::Duration::seconds(self.config.window_duration_secs() as i64);
        let recent = self
            .store
            .count_recent_jobs_for_user(user_id, window_start)?;
        if recent >= self.config.max_jobs_per_window() {
            return Err(SchedulerError::QuotaExceeded(
                "job creation rate exceeded".to_string(),
            ));
        }
        Ok(())
    }
}

fn compute_initial_run(
    request: &CreateJobRequest,
) -> SchedulerResult<chrono::DateTime<chrono::Utc>> {
    compute_next_run_for(request.schedule_type, &request.schedule_expr)
}

pub fn compute_next_run_for(
    schedule_type: ScheduleType,
    schedule_expr: &str,
) -> SchedulerResult<chrono::DateTime<chrono::Utc>> {
    match schedule_type {
        ScheduleType::Interval => {
            let secs = schedule_expr.parse::<u64>().map_err(|_| {
                SchedulerError::InvalidSchedule(
                    "interval schedule_expr must be seconds".to_string(),
                )
            })?;
            Ok(chrono::Utc::now() + chrono::Duration::seconds(secs as i64))
        }
        ScheduleType::Once => Ok(chrono::Utc::now()),
        ScheduleType::Cron => compute_cron_initial_run(schedule_expr),
    }
}

const MIN_CRON_INTERVAL_SECS: i64 = 60;

fn compute_cron_initial_run(expr: &str) -> SchedulerResult<chrono::DateTime<chrono::Utc>> {
    let (schedule, timezone) = parse_cron_schedule(expr)?;
    let now = chrono::Utc::now();
    let next = next_cron_occurrence_with_schedule(&schedule, timezone, now)?;
    let follow = next_cron_occurrence_with_schedule(&schedule, timezone, next)?;
    let delta = follow - next;
    if delta < chrono::Duration::seconds(MIN_CRON_INTERVAL_SECS) {
        return Err(SchedulerError::InvalidSchedule(
            "cron interval must be at least 60 seconds".to_string(),
        ));
    }
    Ok(next)
}

pub fn next_cron_occurrence(
    expr: &str,
    after: chrono::DateTime<chrono::Utc>,
) -> SchedulerResult<chrono::DateTime<chrono::Utc>> {
    let (schedule, timezone) = parse_cron_schedule(expr)?;
    next_cron_occurrence_with_schedule(&schedule, timezone, after)
}

fn parse_cron_schedule(
    expr: &str,
) -> SchedulerResult<(cron::Schedule, chrono_tz::Tz)> {
    let (timezone, raw_expr) = parse_cron_with_timezone(expr)?;
    let schedule = raw_expr
        .parse::<cron::Schedule>()
        .map_err(|err| SchedulerError::InvalidSchedule(err.to_string()))?;
    Ok((schedule, timezone))
}

fn next_cron_occurrence_with_schedule(
    schedule: &cron::Schedule,
    timezone: chrono_tz::Tz,
    after: chrono::DateTime<chrono::Utc>,
) -> SchedulerResult<chrono::DateTime<chrono::Utc>> {
    let localized = after.with_timezone(&timezone);
    schedule
        .after(&localized)
        .next()
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .ok_or_else(|| {
            SchedulerError::InvalidSchedule("cron has no future occurrences".to_string())
        })
}

fn parse_cron_with_timezone(expr: &str) -> SchedulerResult<(chrono_tz::Tz, &str)> {
    let trimmed = expr.trim();
    let mut parts = trimmed.splitn(2, '|');
    let first = parts.next().unwrap_or_default().trim();
    let second = parts.next().map(str::trim);
    if let Some(schedule_expr) = second {
        let tz = first
            .parse::<chrono_tz::Tz>()
            .map_err(|_| SchedulerError::InvalidSchedule("invalid cron timezone".to_string()))?;
        if schedule_expr.is_empty() {
            return Err(SchedulerError::InvalidSchedule(
                "cron expression missing".to_string(),
            ));
        }
        Ok((tz, schedule_expr))
    } else {
        Ok((chrono_tz::UTC, first))
    }
}

#[cfg(test)]
mod tests {
    use super::{compute_initial_run, next_cron_occurrence};
    use crate::scheduler::job::{CreateJobRequest, Principal, PrincipalType, ScheduleType};

    #[test]
    fn compute_initial_run_interval() {
        let request = CreateJobRequest {
            name: "interval".to_string(),
            schedule_type: ScheduleType::Interval,
            schedule_expr: "10".to_string(),
            task_prompt: "ping".to_string(),
            session_id: None,
            user_id: "user".to_string(),
            channel_id: None,
            capabilities: crate::kernel::permissions::CapabilitySet::empty(),
            creator: Principal {
                principal_type: PrincipalType::User,
                id: "user".to_string(),
            },
            enabled: true,
            max_executions: None,
            created_by_system: false,
            metadata: None,
        };
        let next = compute_initial_run(&request).unwrap();
        assert!(next > chrono::Utc::now());
    }

    #[test]
    fn compute_initial_run_cron_respects_min_interval() {
        let request = CreateJobRequest {
            name: "cron".to_string(),
            schedule_type: ScheduleType::Cron,
            schedule_expr: "* * * * * *".to_string(),
            task_prompt: "ping".to_string(),
            session_id: None,
            user_id: "user".to_string(),
            channel_id: None,
            capabilities: crate::kernel::permissions::CapabilitySet::empty(),
            creator: Principal {
                principal_type: PrincipalType::User,
                id: "user".to_string(),
            },
            enabled: true,
            max_executions: None,
            created_by_system: false,
            metadata: None,
        };
        let err = compute_initial_run(&request).unwrap_err();
        assert!(err.to_string().contains("cron interval"));
    }

    #[test]
    fn next_cron_occurrence_supports_timezone_prefix() {
        let expr = "America/New_York|0 */2 * * * *";
        let now = chrono::Utc::now();
        let next = next_cron_occurrence(expr, now).unwrap();
        assert!(next > now);
    }
}
