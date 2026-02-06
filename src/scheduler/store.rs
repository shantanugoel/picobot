use rusqlite::{Connection, params};

use crate::scheduler::error::{SchedulerError, SchedulerResult};
use crate::scheduler::job::{
    CreateJobRequest, ExecutionStatus, JobExecution, Principal, PrincipalType, ScheduleType,
    ScheduledJob,
};
use crate::session::db::SqliteStore;
use crate::session::error::SessionDbError;

#[derive(Debug, Clone)]
pub struct ScheduleStore {
    store: SqliteStore,
}

impl ScheduleStore {
    pub fn new(store: SqliteStore) -> Self {
        Self { store }
    }

    #[allow(dead_code)]
    pub fn store(&self) -> &SqliteStore {
        &self.store
    }

    pub fn create_job(
        &self,
        request: CreateJobRequest,
        next_run_at: chrono::DateTime<chrono::Utc>,
    ) -> SchedulerResult<ScheduledJob> {
        let created_by_system = matches!(request.creator.principal_type, PrincipalType::System);
        let job = ScheduledJob {
            id: uuid::Uuid::new_v4().to_string(),
            name: request.name,
            schedule_type: request.schedule_type,
            schedule_expr: request.schedule_expr,
            task_prompt: request.task_prompt,
            session_id: request.session_id,
            user_id: request.user_id,
            channel_id: request.channel_id,
            capabilities: request.capabilities,
            creator: request.creator,
            enabled: request.enabled,
            max_executions: request.max_executions,
            created_by_system,
            execution_count: 0,
            claimed_at: None,
            claim_id: None,
            claim_expires_at: None,
            last_run_at: None,
            next_run_at,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            consecutive_failures: 0,
            last_error: None,
            backoff_until: None,
            metadata: request.metadata,
        };
        self.store
            .with_connection(|conn| insert_job(conn, &job))
            .map_err(|err| SchedulerError::Store(err.to_string()))?;
        Ok(job)
    }

    pub fn list_jobs_by_user(&self, user_id: &str) -> SchedulerResult<Vec<ScheduledJob>> {
        self.store
            .with_connection(|conn| load_jobs_by_user(conn, user_id))
            .map_err(|err| SchedulerError::Store(err.to_string()))
    }

    pub fn list_jobs_by_user_with_session(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> SchedulerResult<Vec<ScheduledJob>> {
        self.store
            .with_connection(|conn| load_jobs_by_user_with_session(conn, user_id, session_id))
            .map_err(|err| SchedulerError::Store(err.to_string()))
    }

    #[allow(dead_code)]
    pub fn list_jobs(&self) -> SchedulerResult<Vec<ScheduledJob>> {
        self.store
            .with_connection(load_jobs)
            .map_err(|err| SchedulerError::Store(err.to_string()))
    }

    pub fn get_job(&self, id: &str) -> SchedulerResult<Option<ScheduledJob>> {
        self.store
            .with_connection(|conn| load_job(conn, id))
            .map_err(|err| SchedulerError::Store(err.to_string()))
    }

    pub fn update_job(&self, job: &ScheduledJob) -> SchedulerResult<()> {
        self.store
            .with_connection(|conn| insert_job(conn, job))
            .map_err(|err| SchedulerError::Store(err.to_string()))
    }

    #[allow(dead_code)]
    pub fn delete_job(&self, id: &str) -> SchedulerResult<()> {
        self.store
            .with_connection(|conn| {
                conn.execute("DELETE FROM schedules WHERE id = ?1", params![id])
                    .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
                Ok(())
            })
            .map_err(|err| SchedulerError::Store(err.to_string()))
    }

    pub fn count_jobs_for_user(&self, user_id: &str) -> SchedulerResult<u32> {
        self.store
            .with_connection(|conn| {
                let mut stmt = conn
                    .prepare("SELECT COUNT(*) FROM schedules WHERE user_id = ?1")
                    .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
                let count: i64 = stmt
                    .query_row([user_id], |row| row.get(0))
                    .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
                Ok(count as u32)
            })
            .map_err(|err| SchedulerError::Store(err.to_string()))
    }

    pub fn count_recent_jobs_for_user(
        &self,
        user_id: &str,
        window_start: chrono::DateTime<chrono::Utc>,
    ) -> SchedulerResult<u32> {
        let window_start = window_start.to_rfc3339();
        self.store
            .with_connection(|conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT COUNT(*) FROM schedules WHERE user_id = ?1 AND created_at >= ?2",
                    )
                    .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
                let count: i64 = stmt
                    .query_row(params![user_id, window_start], |row| row.get(0))
                    .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
                Ok(count as u32)
            })
            .map_err(|err| SchedulerError::Store(err.to_string()))
    }

    pub fn claim_due_jobs(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        limit: usize,
        claim_id: &str,
        lease_secs: u64,
    ) -> SchedulerResult<Vec<ScheduledJob>> {
        let now_value = now.to_rfc3339();
        let expires_at = (now + chrono::Duration::seconds(lease_secs as i64)).to_rfc3339();
        self.store
            .with_connection(|conn| {
                conn.execute("BEGIN IMMEDIATE", [])
                    .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
                let mut stmt = conn
                    .prepare(
                        "SELECT id FROM schedules
                         WHERE enabled = 1
                           AND next_run_at <= ?1
                           AND (backoff_until IS NULL OR backoff_until <= ?1)
                           AND (claim_expires_at IS NULL OR claim_expires_at <= ?1)
                           AND (max_executions IS NULL OR execution_count < max_executions)
                         ORDER BY next_run_at ASC
                         LIMIT ?2",
                    )
                    .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
                let ids = stmt
                    .query_map(params![now_value, limit as i64], |row| row.get::<_, String>(0))
                    .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
                let mut claimed_ids = Vec::new();
                for id in ids {
                    let id = id.map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
                    let updated = conn
                        .execute(
                            "UPDATE schedules
                             SET claimed_at = ?1, claim_id = ?2, claim_expires_at = ?3, updated_at = ?4
                             WHERE id = ?5
                               AND (claim_expires_at IS NULL OR claim_expires_at <= ?1)
                               AND (backoff_until IS NULL OR backoff_until <= ?1)
                               AND (max_executions IS NULL OR execution_count < max_executions)
                               AND enabled = 1",
                            params![now_value, claim_id, expires_at, now_value, id],
                        )
                        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
                    if updated == 1 {
                        claimed_ids.push(id);
                    }
                }
                conn.execute("COMMIT", [])
                    .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
                let mut jobs = Vec::new();
                for id in claimed_ids {
                    if let Some(job) = load_job(conn, &id)? {
                        jobs.push(job);
                    }
                }
                Ok(jobs)
            })
            .map_err(|err| SchedulerError::Store(err.to_string()))
    }

    pub fn release_claim(&self, id: &str, claim_id: &str) -> SchedulerResult<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.store
            .with_connection(|conn| {
                conn.execute(
                    "UPDATE schedules
                     SET claimed_at = NULL, claim_id = NULL, claim_expires_at = NULL, updated_at = ?1
                     WHERE id = ?2 AND claim_id = ?3",
                    params![now, id, claim_id],
                )
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
                Ok(())
            })
            .map_err(|err| SchedulerError::Store(err.to_string()))
    }

    pub fn insert_execution(&self, execution: &JobExecution) -> SchedulerResult<()> {
        self.store
            .with_connection(|conn| insert_execution(conn, execution))
            .map_err(|err| SchedulerError::Store(err.to_string()))
    }

    pub fn update_execution(&self, execution: &JobExecution) -> SchedulerResult<()> {
        self.store
            .with_connection(|conn| update_execution(conn, execution))
            .map_err(|err| SchedulerError::Store(err.to_string()))
    }

    #[allow(dead_code)]
    pub fn list_executions_for_job(
        &self,
        job_id: &str,
        limit: usize,
        offset: usize,
    ) -> SchedulerResult<Vec<JobExecution>> {
        self.store
            .with_connection(|conn| load_executions_for_job(conn, job_id, limit, offset))
            .map_err(|err| SchedulerError::Store(err.to_string()))
    }

    #[allow(dead_code)]
    pub fn list_all_executions(&self) -> SchedulerResult<Vec<JobExecution>> {
        self.store
            .with_connection(load_all_executions)
            .map_err(|err| SchedulerError::Store(err.to_string()))
    }
}

fn insert_job(conn: &Connection, job: &ScheduledJob) -> Result<(), SessionDbError> {
    let capabilities_json = serde_json::to_string(&job.capabilities)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let creator_json = serde_json::to_string(&job.creator)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let metadata_json = job
        .metadata
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    conn.execute(
        "INSERT OR REPLACE INTO schedules
         (id, name, schedule_type, schedule_expr, task_prompt, session_id, user_id, channel_id,
          capabilities_json, creator_principal, enabled, max_executions, execution_count,
          claimed_at, claim_id, claim_expires_at, last_run_at, next_run_at, created_at, updated_at,
          consecutive_failures, last_error, backoff_until, metadata_json, created_by_system)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                 ?9, ?10, ?11, ?12, ?13,
                 ?14, ?15, ?16, ?17, ?18, ?19, ?20,
                 ?21, ?22, ?23, ?24, ?25)",
        params![
            job.id,
            job.name,
            schedule_type_to_str(job.schedule_type),
            job.schedule_expr,
            job.task_prompt,
            job.session_id,
            job.user_id,
            job.channel_id,
            capabilities_json,
            creator_json,
            if job.enabled { 1 } else { 0 },
            job.max_executions.map(|value| value as i64),
            job.execution_count as i64,
            job.claimed_at.map(|value| value.to_rfc3339()),
            job.claim_id,
            job.claim_expires_at.map(|value| value.to_rfc3339()),
            job.last_run_at.map(|value| value.to_rfc3339()),
            job.next_run_at.to_rfc3339(),
            job.created_at.to_rfc3339(),
            job.updated_at.to_rfc3339(),
            job.consecutive_failures as i64,
            job.last_error,
            job.backoff_until.map(|value| value.to_rfc3339()),
            metadata_json,
            if job.created_by_system { 1 } else { 0 },
        ],
    )
    .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    Ok(())
}

fn load_jobs_by_user(
    conn: &Connection,
    user_id: &str,
) -> Result<Vec<ScheduledJob>, SessionDbError> {
    let mut stmt = conn
        .prepare("SELECT id FROM schedules WHERE user_id = ?1 ORDER BY created_at DESC")
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let rows = stmt
        .query_map([user_id], |row| row.get::<_, String>(0))
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let mut jobs = Vec::new();
    for row in rows {
        let id = row.map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
        if let Some(job) = load_job(conn, &id)? {
            jobs.push(job);
        }
    }
    Ok(jobs)
}

fn load_jobs_by_user_with_session(
    conn: &Connection,
    user_id: &str,
    session_id: &str,
) -> Result<Vec<ScheduledJob>, SessionDbError> {
    let mut stmt = conn
        .prepare(
            "SELECT id FROM schedules WHERE user_id = ?1 AND session_id = ?2 ORDER BY created_at DESC",
        )
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let rows = stmt
        .query_map(params![user_id, session_id], |row| row.get::<_, String>(0))
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let mut jobs = Vec::new();
    for row in rows {
        let id = row.map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
        if let Some(job) = load_job(conn, &id)? {
            jobs.push(job);
        }
    }
    Ok(jobs)
}

#[allow(dead_code)]
fn load_jobs(conn: &Connection) -> Result<Vec<ScheduledJob>, SessionDbError> {
    let mut stmt = conn
        .prepare("SELECT id FROM schedules ORDER BY created_at DESC")
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let mut jobs = Vec::new();
    for row in rows {
        let id = row.map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
        if let Some(job) = load_job(conn, &id)? {
            jobs.push(job);
        }
    }
    Ok(jobs)
}

fn load_job(conn: &Connection, id: &str) -> Result<Option<ScheduledJob>, SessionDbError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, schedule_type, schedule_expr, task_prompt, session_id, user_id, channel_id,
                    capabilities_json, creator_principal, enabled, max_executions, execution_count,
                    claimed_at, claim_id, claim_expires_at, last_run_at, next_run_at, created_at, updated_at,
                    consecutive_failures, last_error, backoff_until, metadata_json, created_by_system
             FROM schedules WHERE id = ?1",
        )
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let mut rows = stmt
        .query(params![id])
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let row = match rows
        .next()
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?
    {
        Some(row) => row,
        None => return Ok(None),
    };
    let schedule_type: String = row
        .get(2)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let schedule_type = parse_schedule_type(&schedule_type)?;
    let capabilities_json: String = row
        .get(8)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let capabilities = serde_json::from_str(&capabilities_json)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let creator_json: String = row
        .get(9)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let creator: Principal = serde_json::from_str(&creator_json)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let metadata_json: Option<String> = row
        .get(23)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let created_by_system: i64 = row
        .get(24)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let metadata = metadata_json
        .map(|value| serde_json::from_str(&value))
        .transpose()
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    Ok(Some(ScheduledJob {
        id: row
            .get(0)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        name: row
            .get(1)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        schedule_type,
        schedule_expr: row
            .get(3)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        task_prompt: row
            .get(4)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        session_id: row
            .get(5)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        user_id: row
            .get(6)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        channel_id: row
            .get(7)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        capabilities,
        creator,
        enabled: row
            .get::<_, i64>(10)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?
            != 0,
        max_executions: row
            .get::<_, Option<i64>>(11)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?
            .map(|value| value as u32),
        execution_count: row
            .get::<_, i64>(12)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?
            as u32,
        claimed_at: parse_datetime(
            row.get(13)
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        ),
        claim_id: row
            .get(14)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        claim_expires_at: parse_datetime(
            row.get(15)
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        ),
        last_run_at: parse_datetime(
            row.get(16)
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        ),
        next_run_at: parse_datetime(
            row.get(17)
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        )
        .ok_or_else(|| SessionDbError::QueryFailed("missing next_run_at".to_string()))?,
        created_at: parse_datetime(
            row.get(18)
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        )
        .ok_or_else(|| SessionDbError::QueryFailed("missing created_at".to_string()))?,
        updated_at: parse_datetime(
            row.get(19)
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        )
        .ok_or_else(|| SessionDbError::QueryFailed("missing updated_at".to_string()))?,
        consecutive_failures: row
            .get::<_, i64>(20)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?
            as u32,
        last_error: row
            .get(21)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        backoff_until: parse_datetime(
            row.get(22)
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        ),
        metadata,
        created_by_system: created_by_system != 0,
    }))
}

fn insert_execution(conn: &Connection, execution: &JobExecution) -> Result<(), SessionDbError> {
    conn.execute(
        "INSERT OR REPLACE INTO schedule_executions
         (id, job_id, started_at, completed_at, status, result_summary, error, execution_time_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            execution.id,
            execution.job_id,
            execution.started_at.to_rfc3339(),
            execution.completed_at.map(|value| value.to_rfc3339()),
            execution_status_to_str(execution.status),
            execution.result_summary,
            execution.error,
            execution.execution_time_ms,
        ],
    )
    .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    Ok(())
}

fn update_execution(conn: &Connection, execution: &JobExecution) -> Result<(), SessionDbError> {
    conn.execute(
        "UPDATE schedule_executions
         SET completed_at = ?1, status = ?2, result_summary = ?3, error = ?4, execution_time_ms = ?5
         WHERE id = ?6",
        params![
            execution.completed_at.map(|value| value.to_rfc3339()),
            execution_status_to_str(execution.status),
            execution.result_summary,
            execution.error,
            execution.execution_time_ms,
            execution.id,
        ],
    )
    .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    Ok(())
}

#[allow(dead_code)]
fn load_executions_for_job(
    conn: &Connection,
    job_id: &str,
    limit: usize,
    offset: usize,
) -> Result<Vec<JobExecution>, SessionDbError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, job_id, started_at, completed_at, status, result_summary, error, execution_time_ms
             FROM schedule_executions
             WHERE job_id = ?1
             ORDER BY started_at DESC
             LIMIT ?2 OFFSET ?3",
        )
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let rows = stmt
        .query_map(params![job_id, limit as i64, offset as i64], |row| {
            let status: String = row.get(4)?;
            let started_at =
                parse_datetime(row.get(2)?).ok_or_else(|| rusqlite::Error::InvalidQuery)?;
            let status =
                parse_execution_status(&status).map_err(|_err| rusqlite::Error::InvalidQuery)?;
            Ok(JobExecution {
                id: row.get(0)?,
                job_id: row.get(1)?,
                started_at,
                completed_at: parse_datetime(row.get(3)?),
                status,
                result_summary: row.get(5)?,
                error: row.get(6)?,
                execution_time_ms: row.get(7)?,
            })
        })
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let mut executions = Vec::new();
    for row in rows {
        executions.push(row.map_err(|err| SessionDbError::QueryFailed(err.to_string()))?);
    }
    Ok(executions)
}

#[allow(dead_code)]
fn load_all_executions(conn: &Connection) -> Result<Vec<JobExecution>, SessionDbError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, job_id, started_at, completed_at, status, result_summary, error, execution_time_ms
             FROM schedule_executions
             ORDER BY started_at DESC",
        )
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let rows = stmt
        .query_map([], |row| {
            let status: String = row.get(4)?;
            let started_at =
                parse_datetime(row.get(2)?).ok_or_else(|| rusqlite::Error::InvalidQuery)?;
            let status =
                parse_execution_status(&status).map_err(|_err| rusqlite::Error::InvalidQuery)?;
            Ok(JobExecution {
                id: row.get(0)?,
                job_id: row.get(1)?,
                started_at,
                completed_at: parse_datetime(row.get(3)?),
                status,
                result_summary: row.get(5)?,
                error: row.get(6)?,
                execution_time_ms: row.get(7)?,
            })
        })
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let mut executions = Vec::new();
    for row in rows {
        executions.push(row.map_err(|err| SessionDbError::QueryFailed(err.to_string()))?);
    }
    Ok(executions)
}

fn parse_datetime(value: Option<String>) -> Option<chrono::DateTime<chrono::Utc>> {
    value
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(&value).ok())
        .map(|value| value.with_timezone(&chrono::Utc))
}

fn schedule_type_to_str(value: ScheduleType) -> &'static str {
    match value {
        ScheduleType::Interval => "interval",
        ScheduleType::Once => "once",
        ScheduleType::Cron => "cron",
    }
}

fn execution_status_to_str(value: ExecutionStatus) -> &'static str {
    match value {
        ExecutionStatus::Running => "running",
        ExecutionStatus::Completed => "completed",
        ExecutionStatus::Failed => "failed",
        ExecutionStatus::Timeout => "timeout",
        ExecutionStatus::Cancelled => "cancelled",
    }
}

fn parse_schedule_type(value: &str) -> Result<ScheduleType, SessionDbError> {
    match value {
        "interval" => Ok(ScheduleType::Interval),
        "once" => Ok(ScheduleType::Once),
        "cron" => Ok(ScheduleType::Cron),
        _ => Err(SessionDbError::QueryFailed(
            "invalid schedule type".to_string(),
        )),
    }
}

#[allow(dead_code)]
fn parse_execution_status(value: &str) -> Result<ExecutionStatus, SessionDbError> {
    match value {
        "running" => Ok(ExecutionStatus::Running),
        "completed" => Ok(ExecutionStatus::Completed),
        "failed" => Ok(ExecutionStatus::Failed),
        "timeout" => Ok(ExecutionStatus::Timeout),
        "cancelled" => Ok(ExecutionStatus::Cancelled),
        _ => Err(SessionDbError::QueryFailed(
            "invalid execution status".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{ScheduleStore, parse_schedule_type};
    use crate::session::db::SqliteStore;

    #[test]
    fn parse_schedule_type_accepts_values() {
        assert!(parse_schedule_type("interval").is_ok());
        assert!(parse_schedule_type("once").is_ok());
        assert!(parse_schedule_type("cron").is_ok());
    }

    #[test]
    fn claim_due_jobs_is_atomic() {
        let dir = std::env::temp_dir().join(format!("picobot-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = SqliteStore::new(dir.join("scheduler.db").to_string_lossy().to_string());
        store.touch().unwrap();
        let schedule_store = ScheduleStore::new(store.clone());

        let user_id = "user".to_string();
        let capabilities = crate::kernel::permissions::CapabilitySet::empty();
        let creator = crate::scheduler::job::Principal {
            principal_type: crate::scheduler::job::PrincipalType::User,
            id: user_id.clone(),
        };
        let request = crate::scheduler::job::CreateJobRequest {
            name: "job".to_string(),
            schedule_type: crate::scheduler::job::ScheduleType::Interval,
            schedule_expr: "1".to_string(),
            task_prompt: "ping".to_string(),
            session_id: None,
            user_id: user_id.clone(),
            channel_id: None,
            capabilities,
            creator,
            enabled: true,
            max_executions: None,
            created_by_system: false,
            metadata: None,
        };
        let now = chrono::Utc::now();
        schedule_store.create_job(request, now).unwrap();
        let claim_id = uuid::Uuid::new_v4().to_string();
        let claimed = schedule_store
            .claim_due_jobs(now, 1, &claim_id, 30)
            .unwrap();
        assert_eq!(claimed.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
