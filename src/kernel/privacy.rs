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

#[cfg(test)]
mod tests {
    use super::{PrivacyController, PurgeScope};
    use crate::channels::adapter::ChannelType;
    use crate::channels::permissions::ChannelPermissionProfile;
    use crate::kernel::context::ToolContext;
    use crate::kernel::permissions::{CapabilitySet, PermissionTier};
    use crate::session::db::SqliteStore;
    use crate::session::persistent_manager::PersistentSessionManager;
    use std::sync::Arc;
    use uuid::Uuid;

    fn temp_store() -> (SqliteStore, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("picobot-privacy-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("conversations.db");
        let store = SqliteStore::new(path.to_string_lossy().to_string());
        store.touch().unwrap();
        (store, dir)
    }

    fn profile() -> ChannelPermissionProfile {
        ChannelPermissionProfile {
            tier: PermissionTier::UserGrantable,
            pre_authorized: Vec::new(),
            max_allowed: Vec::new(),
            allow_user_prompts: true,
            prompt_timeout_secs: 120,
        }
    }

    #[test]
    fn purge_session_removes_session_and_messages() {
        let (store, dir) = temp_store();
        let manager = Arc::new(PersistentSessionManager::new(store.clone()));
        let session = manager
            .create_session(
                "session-1".to_string(),
                ChannelType::Api,
                "api".to_string(),
                "api:user".to_string(),
                &profile(),
            )
            .unwrap();
        let mut session = session;
        session
            .conversation
            .push(crate::models::types::Message::user("hello"));
        manager.update_session(&session).unwrap();

        let controller = PrivacyController::new(Arc::clone(&manager));
        let ctx = ToolContext {
            working_dir: std::path::PathBuf::from("/"),
            capabilities: Arc::new(CapabilitySet::empty()),
            user_id: Some("api:user".to_string()),
            session_id: Some("session-1".to_string()),
            scheduler: Arc::new(std::sync::RwLock::new(None)),
            log_model_requests: false,
            include_tool_messages: true,
        };
        controller.purge(&ctx, PurgeScope::Session, None).unwrap();
        assert!(manager.get_session("session-1").unwrap().is_none());

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn purge_user_removes_sessions_and_memories() {
        let (store, dir) = temp_store();
        let manager = Arc::new(PersistentSessionManager::new(store.clone()));
        let _ = manager
            .create_session(
                "session-2".to_string(),
                ChannelType::Api,
                "api".to_string(),
                "api:user2".to_string(),
                &profile(),
            )
            .unwrap();
        store
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO user_memories (user_id, key, content, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        "api:user2",
                        "favorite",
                        "blue",
                        chrono::Utc::now().to_rfc3339(),
                        chrono::Utc::now().to_rfc3339()
                    ],
                )
                .unwrap();
                Ok(())
            })
            .unwrap();

        let controller = PrivacyController::new(Arc::clone(&manager));
        let ctx = ToolContext {
            working_dir: std::path::PathBuf::from("/"),
            capabilities: Arc::new(CapabilitySet::empty()),
            user_id: Some("api:user2".to_string()),
            session_id: None,
            scheduler: Arc::new(std::sync::RwLock::new(None)),
            log_model_requests: false,
            include_tool_messages: true,
        };
        controller.purge(&ctx, PurgeScope::User, None).unwrap();
        assert!(manager.get_session("session-2").unwrap().is_none());
        let memories = store
            .with_connection(|conn| {
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM user_memories WHERE user_id = ?1",
                        ["api:user2"],
                        |row| row.get(0),
                    )
                    .unwrap();
                Ok(count)
            })
            .unwrap();
        assert_eq!(memories, 0);

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn purge_older_than_removes_messages_only() {
        let (store, dir) = temp_store();
        let manager = Arc::new(PersistentSessionManager::new(store.clone()));
        let session = manager
            .create_session(
                "session-3".to_string(),
                ChannelType::Api,
                "api".to_string(),
                "api:user3".to_string(),
                &profile(),
            )
            .unwrap();
        store
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO messages (session_id, message_type, content, created_at, seq_order)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        session.id,
                        "user",
                        "old",
                        "2000-01-01T00:00:00Z",
                        0
                    ],
                )
                .unwrap();
                Ok(())
            })
            .unwrap();

        let controller = PrivacyController::new(Arc::clone(&manager));
        let ctx = ToolContext {
            working_dir: std::path::PathBuf::from("/"),
            capabilities: Arc::new(CapabilitySet::empty()),
            user_id: Some("api:user3".to_string()),
            session_id: Some("session-3".to_string()),
            scheduler: Arc::new(std::sync::RwLock::new(None)),
            log_model_requests: false,
            include_tool_messages: true,
        };
        controller
            .purge(&ctx, PurgeScope::OlderThanDays, Some(1))
            .unwrap();
        let remaining = store
            .with_connection(|conn| {
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
                        ["session-3"],
                        |row| row.get(0),
                    )
                    .unwrap();
                Ok(count)
            })
            .unwrap();
        assert_eq!(remaining, 0);

        std::fs::remove_dir_all(dir).ok();
    }
}
