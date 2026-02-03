use std::sync::Arc;

use crate::session::manager::SessionManager;

pub fn render_metrics(sessions: &Arc<SessionManager>) -> String {
    let summaries = sessions.list_sessions();
    let total = summaries.len();
    let active = summaries
        .iter()
        .filter(|session| session.state == crate::session::manager::SessionState::Active)
        .count();
    format!(
        "picobot_sessions_total {}\npicobot_sessions_active {}\n",
        total, active
    )
}
