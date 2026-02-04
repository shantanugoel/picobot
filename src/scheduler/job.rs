use serde::{Deserialize, Serialize};

use crate::kernel::permissions::CapabilitySet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleType {
    Interval,
    Once,
    Cron,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrincipalType {
    User,
    System,
    Admin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    pub principal_type: PrincipalType,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionStatus {
    Running,
    Completed,
    Failed,
    Timeout,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledJob {
    pub id: String,
    pub name: String,
    pub schedule_type: ScheduleType,
    pub schedule_expr: String,
    pub task_prompt: String,
    pub session_id: Option<String>,
    pub user_id: String,
    pub channel_id: Option<String>,
    pub capabilities: CapabilitySet,
    pub creator: Principal,
    pub enabled: bool,
    pub max_executions: Option<u32>,
    #[serde(default)]
    pub created_by_system: bool,
    pub execution_count: u32,
    pub claimed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub claim_id: Option<String>,
    pub claim_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_run_at: Option<chrono::DateTime<chrono::Utc>>,
    pub next_run_at: chrono::DateTime<chrono::Utc>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub consecutive_failures: u32,
    pub last_error: Option<String>,
    pub backoff_until: Option<chrono::DateTime<chrono::Utc>>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobExecution {
    pub id: String,
    pub job_id: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub status: ExecutionStatus,
    pub result_summary: Option<String>,
    pub error: Option<String>,
    pub execution_time_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateJobRequest {
    pub name: String,
    pub schedule_type: ScheduleType,
    pub schedule_expr: String,
    pub task_prompt: String,
    pub session_id: Option<String>,
    pub user_id: String,
    pub channel_id: Option<String>,
    pub capabilities: CapabilitySet,
    pub creator: Principal,
    pub enabled: bool,
    pub max_executions: Option<u32>,
    pub created_by_system: bool,
    pub metadata: Option<serde_json::Value>,
}

impl ScheduledJob {
    pub fn schedule_interval_seconds(&self) -> Option<u64> {
        match self.schedule_type {
            ScheduleType::Interval => self.schedule_expr.parse::<u64>().ok(),
            ScheduleType::Once => None,
            ScheduleType::Cron => None,
        }
    }
}
