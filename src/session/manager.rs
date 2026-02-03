use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::channels::adapter::ChannelType;
use crate::channels::permissions::ChannelPermissionProfile;
use crate::kernel::permissions::CapabilitySet;
use crate::models::types::Message;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub channel_type: ChannelType,
    pub channel_id: String,
    pub user_id: String,
    pub conversation: Vec<Message>,
    pub permissions: CapabilitySet,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_active: chrono::DateTime<chrono::Utc>,
    pub state: SessionState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    Active,
    AwaitingPermission { tool: String, request_id: String },
    Idle,
    Terminated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub channel_id: String,
    pub user_id: String,
    pub last_active: chrono::DateTime<chrono::Utc>,
    pub state: SessionState,
}

#[derive(Debug, Default, Clone)]
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_session(
        &self,
        id: String,
        channel_type: ChannelType,
        channel_id: String,
        user_id: String,
        profile: &ChannelPermissionProfile,
    ) -> Session {
        let now = chrono::Utc::now();
        let session = Session {
            id: id.clone(),
            channel_type,
            channel_id,
            user_id,
            conversation: Vec::new(),
            permissions: profile.grants(),
            created_at: now,
            last_active: now,
            state: SessionState::Active,
        };
        if let Ok(mut sessions) = self.sessions.write() {
            sessions.insert(id, session.clone());
        }
        session
    }

    pub fn get_session(&self, id: &str) -> Option<Session> {
        self.sessions.read().ok()?.get(id).cloned()
    }

    pub fn update_session(&self, session: Session) {
        if let Ok(mut sessions) = self.sessions.write() {
            sessions.insert(session.id.clone(), session);
        }
    }

    pub fn delete_session(&self, id: &str) {
        if let Ok(mut sessions) = self.sessions.write() {
            sessions.remove(id);
        }
    }

    pub fn list_sessions(&self) -> Vec<SessionSummary> {
        self.sessions
            .read()
            .map(|sessions| {
                sessions
                    .values()
                    .map(|session| SessionSummary {
                        id: session.id.clone(),
                        channel_id: session.channel_id.clone(),
                        user_id: session.user_id.clone(),
                        last_active: session.last_active,
                        state: session.state.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn all_sessions(&self) -> Vec<Session> {
        self.sessions
            .read()
            .map(|sessions| sessions.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn restore_sessions(&self, sessions: Vec<Session>) {
        if let Ok(mut store) = self.sessions.write() {
            store.clear();
            for session in sessions {
                store.insert(session.id.clone(), session);
            }
        }
    }

    pub fn touch(&self, id: &str) {
        if let Ok(mut sessions) = self.sessions.write()
            && let Some(session) = sessions.get_mut(id)
        {
            session.last_active = chrono::Utc::now();
        }
    }
}
