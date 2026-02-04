use std::sync::Arc;

use crate::delivery::tracking::{DeliveryStatus, DeliveryTracker};
use crate::session::persistent_manager::PersistentSessionManager;

#[derive(Default, Debug, Clone)]
pub struct MetricsSnapshot {
    pub sessions_total: usize,
    pub sessions_active: usize,
    pub sessions_idle: usize,
    pub sessions_awaiting_permission: usize,
    pub sessions_terminated: usize,
    pub deliveries_total: usize,
    pub deliveries_pending: usize,
    pub deliveries_sending: usize,
    pub deliveries_sent: usize,
    pub deliveries_failed: usize,
    pub schedules_total: usize,
    pub schedules_enabled: usize,
    pub schedules_disabled: usize,
    pub schedules_system: usize,
    pub executions_total: usize,
    pub executions_running: usize,
    pub executions_completed: usize,
    pub executions_failed: usize,
    pub executions_timeout: usize,
    pub executions_cancelled: usize,
}

pub fn render_metrics(
    sessions: &Arc<PersistentSessionManager>,
    deliveries: &DeliveryTracker,
    scheduler: Option<&Arc<crate::scheduler::service::SchedulerService>>,
) -> String {
    let snapshot = collect_metrics(sessions, deliveries, scheduler);
    format!(
        "picobot_sessions_total {}\n\
picobot_sessions_active {}\n\
picobot_sessions_idle {}\n\
picobot_sessions_awaiting_permission {}\n\
picobot_sessions_terminated {}\n\
picobot_deliveries_total {}\n\
picobot_deliveries_pending {}\n\
picobot_deliveries_sending {}\n\
picobot_deliveries_sent {}\n\
picobot_deliveries_failed {}\n\
picobot_schedules_total {}\n\
picobot_schedules_enabled {}\n\
picobot_schedules_disabled {}\n\
picobot_schedules_system {}\n\
picobot_executions_total {}\n\
picobot_executions_running {}\n\
picobot_executions_completed {}\n\
picobot_executions_failed {}\n\
picobot_executions_timeout {}\n\
picobot_executions_cancelled {}\n",
        snapshot.sessions_total,
        snapshot.sessions_active,
        snapshot.sessions_idle,
        snapshot.sessions_awaiting_permission,
        snapshot.sessions_terminated,
        snapshot.deliveries_total,
        snapshot.deliveries_pending,
        snapshot.deliveries_sending,
        snapshot.deliveries_sent,
        snapshot.deliveries_failed,
        snapshot.schedules_total,
        snapshot.schedules_enabled,
        snapshot.schedules_disabled,
        snapshot.schedules_system,
        snapshot.executions_total,
        snapshot.executions_running,
        snapshot.executions_completed,
        snapshot.executions_failed,
        snapshot.executions_timeout,
        snapshot.executions_cancelled,
    )
}

pub fn collect_metrics(
    sessions: &Arc<PersistentSessionManager>,
    deliveries: &DeliveryTracker,
    scheduler: Option<&Arc<crate::scheduler::service::SchedulerService>>,
) -> MetricsSnapshot {
    let summaries = sessions.list_sessions().unwrap_or_default();
    let mut snapshot = MetricsSnapshot {
        sessions_total: summaries.len(),
        ..MetricsSnapshot::default()
    };
    for session in summaries {
        match session.state {
            crate::session::manager::SessionState::Active => snapshot.sessions_active += 1,
            crate::session::manager::SessionState::Idle => snapshot.sessions_idle += 1,
            crate::session::manager::SessionState::AwaitingPermission { .. } => {
                snapshot.sessions_awaiting_permission += 1;
            }
            crate::session::manager::SessionState::Terminated => {
                snapshot.sessions_terminated += 1;
            }
        }
    }
    let deliveries_snapshot = deliveries.snapshot();
    snapshot.deliveries_total = deliveries_snapshot.len();
    for delivery in deliveries_snapshot {
        match delivery.status {
            DeliveryStatus::Pending => snapshot.deliveries_pending += 1,
            DeliveryStatus::Sending => snapshot.deliveries_sending += 1,
            DeliveryStatus::Sent => snapshot.deliveries_sent += 1,
            DeliveryStatus::Failed => snapshot.deliveries_failed += 1,
        }
    }
    if let Some(scheduler) = scheduler {
        if let Ok(jobs) = scheduler.list_jobs() {
            snapshot.schedules_total = jobs.len();
            for job in &jobs {
                if job.enabled {
                    snapshot.schedules_enabled += 1;
                } else {
                    snapshot.schedules_disabled += 1;
                }
                if job.created_by_system {
                    snapshot.schedules_system += 1;
                }
            }
        }
        if let Ok(executions) = scheduler.list_all_executions() {
            snapshot.executions_total = executions.len();
            for execution in executions {
                match execution.status {
                    crate::scheduler::job::ExecutionStatus::Running => {
                        snapshot.executions_running += 1;
                    }
                    crate::scheduler::job::ExecutionStatus::Completed => {
                        snapshot.executions_completed += 1;
                    }
                    crate::scheduler::job::ExecutionStatus::Failed => {
                        snapshot.executions_failed += 1;
                    }
                    crate::scheduler::job::ExecutionStatus::Timeout => {
                        snapshot.executions_timeout += 1;
                    }
                    crate::scheduler::job::ExecutionStatus::Cancelled => {
                        snapshot.executions_cancelled += 1;
                    }
                }
            }
        }
    }
    snapshot
}
