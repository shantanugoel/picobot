use crate::kernel::permissions::Permission;

#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("Store error: {0}")]
    Store(String),
    #[error("Job not found")]
    NotFound,
    #[error("Scheduler disabled")]
    Disabled,
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    #[error("Invalid schedule: {0}")]
    InvalidSchedule(String),
    #[error("Concurrency limit reached")]
    ConcurrencyLimit,
    #[error("Quota exceeded: {0}")]
    QuotaExceeded(String),
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
    #[error("Permission decision required: {0}")]
    PermissionDecisionRequired(String),
    #[error("Missing capability snapshot")]
    MissingCapabilities,
    #[error("Permission denied for tool '{tool}': requires {required:?}")]
    ToolPermissionDenied {
        tool: String,
        required: Vec<Permission>,
    },
}

pub type SchedulerResult<T> = Result<T, SchedulerError>;
