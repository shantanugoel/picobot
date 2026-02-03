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
}

pub fn render_metrics(
    sessions: &Arc<PersistentSessionManager>,
    deliveries: &DeliveryTracker,
) -> String {
    let snapshot = collect_metrics(sessions, deliveries);
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
picobot_deliveries_failed {}\n",
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
    )
}

pub fn collect_metrics(
    sessions: &Arc<PersistentSessionManager>,
    deliveries: &DeliveryTracker,
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
    snapshot
}
