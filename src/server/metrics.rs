use std::sync::Arc;

use crate::session::manager::SessionManager;

#[derive(Default, Debug, Clone)]
pub struct MetricsSnapshot {
    pub sessions_total: usize,
    pub sessions_active: usize,
    pub sessions_idle: usize,
    pub sessions_awaiting_permission: usize,
    pub sessions_terminated: usize,
}

pub fn render_metrics(sessions: &Arc<SessionManager>) -> String {
    let snapshot = collect_metrics(sessions);
    format!(
        "picobot_sessions_total {}\n\
picobot_sessions_active {}\n\
picobot_sessions_idle {}\n\
picobot_sessions_awaiting_permission {}\n\
picobot_sessions_terminated {}\n",
        snapshot.sessions_total,
        snapshot.sessions_active,
        snapshot.sessions_idle,
        snapshot.sessions_awaiting_permission,
        snapshot.sessions_terminated,
    )
}

pub fn collect_metrics(sessions: &Arc<SessionManager>) -> MetricsSnapshot {
    let summaries = sessions.list_sessions();
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
    snapshot
}
