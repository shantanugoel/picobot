use crate::kernel::context::ToolContext;
use crate::session::error::SessionDbResult;
use std::sync::Arc;

use crate::session::persistent_manager::PersistentSessionManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurgeScope {
    Session,
    User,
    OlderThanDays,
}

pub struct PrivacyController {
    sessions: Arc<PersistentSessionManager>,
}

impl PrivacyController {
    pub fn new(sessions: Arc<PersistentSessionManager>) -> Self {
        Self { sessions }
    }

    pub fn purge(
        &self,
        ctx: &ToolContext,
        scope: PurgeScope,
        days: Option<u32>,
    ) -> SessionDbResult<()> {
        let store = self.sessions.store();
        store.with_connection(|conn| match scope {
            PurgeScope::Session => {
                if let Some(session_id) = ctx.session_id.as_ref() {
                    conn.execute("DELETE FROM sessions WHERE id = ?1", [session_id])
                        .map_err(|err| {
                            crate::session::error::SessionDbError::QueryFailed(err.to_string())
                        })?;
                }
                Ok(())
            }
            PurgeScope::User => {
                if let Some(user_id) = ctx.user_id.as_ref() {
                    conn.execute("DELETE FROM sessions WHERE user_id = ?1", [user_id])
                        .map_err(|err| {
                            crate::session::error::SessionDbError::QueryFailed(err.to_string())
                        })?;
                    conn.execute("DELETE FROM user_memories WHERE user_id = ?1", [user_id])
                        .map_err(|err| {
                            crate::session::error::SessionDbError::QueryFailed(err.to_string())
                        })?;
                }
                Ok(())
            }
            PurgeScope::OlderThanDays => {
                let days = days.unwrap_or(0);
                if days == 0 {
                    return Ok(());
                }
                let cutoff = chrono::Utc::now()
                    .checked_sub_signed(chrono::Duration::days(days as i64))
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
                conn.execute("DELETE FROM messages WHERE created_at < ?1", [cutoff])
                    .map_err(|err| {
                        crate::session::error::SessionDbError::QueryFailed(err.to_string())
                    })?;
                Ok(())
            }
        })
    }
}
