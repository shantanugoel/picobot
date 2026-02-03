use rusqlite::{params, Connection};

use crate::channels::adapter::ChannelType;
use crate::channels::permissions::ChannelPermissionProfile;
use crate::kernel::permissions::CapabilitySet;
use crate::models::types::Message;
use crate::session::db::SqliteStore;
use crate::session::error::{SessionDbError, SessionDbResult};
use crate::session::manager::{Session, SessionState, SessionSummary};

#[derive(Debug, Clone)]
pub struct PersistentSessionManager {
    store: SqliteStore,
}

impl PersistentSessionManager {
    pub fn new(store: SqliteStore) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &SqliteStore {
        &self.store
    }

    pub fn create_session(
        &self,
        id: String,
        channel_type: ChannelType,
        channel_id: String,
        user_id: String,
        profile: &ChannelPermissionProfile,
    ) -> SessionDbResult<Session> {
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
        self.store.with_connection(|conn| {
            insert_session(conn, &session)?;
            Ok(())
        })?;
        Ok(session)
    }

    pub fn get_session(&self, id: &str) -> SessionDbResult<Option<Session>> {
        self.store.with_connection(|conn| load_session(conn, id))
    }

    pub fn update_session(&self, session: &Session) -> SessionDbResult<()> {
        self.store.with_connection(|conn| {
            insert_session(conn, session)?;
            Ok(())
        })
    }

    pub fn delete_session(&self, id: &str) -> SessionDbResult<()> {
        self.store.with_connection(|conn| {
            conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
            Ok(())
        })
    }

    pub fn list_sessions(&self) -> SessionDbResult<Vec<SessionSummary>> {
        self.store.with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, channel_id, user_id, last_active, state_json
                     FROM sessions",
                )
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
            let rows = stmt
                .query_map([], |row| {
                    let state_json: String = row.get(4)?;
                    let state: SessionState = serde_json::from_str(&state_json)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?;
                    Ok(SessionSummary {
                        id: row.get(0)?,
                        channel_id: row.get(1)?,
                        user_id: row.get(2)?,
                        last_active: parse_datetime(row.get::<_, String>(3)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        state,
                    })
                })
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
            let mut sessions = Vec::new();
            for row in rows {
                sessions.push(row.map_err(|err| SessionDbError::QueryFailed(err.to_string()))?);
            }
            Ok(sessions)
        })
    }

    pub fn list_sessions_by_user(&self, user_id: &str) -> SessionDbResult<Vec<SessionSummary>> {
        self.store.with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, channel_id, user_id, last_active, state_json
                     FROM sessions WHERE user_id = ?1",
                )
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
            let rows = stmt
                .query_map([user_id], |row| {
                    let state_json: String = row.get(4)?;
                    let state: SessionState = serde_json::from_str(&state_json)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?;
                    Ok(SessionSummary {
                        id: row.get(0)?,
                        channel_id: row.get(1)?,
                        user_id: row.get(2)?,
                        last_active: parse_datetime(row.get::<_, String>(3)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        state,
                    })
                })
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
            let mut sessions = Vec::new();
            for row in rows {
                sessions.push(row.map_err(|err| SessionDbError::QueryFailed(err.to_string()))?);
            }
            Ok(sessions)
        })
    }

    pub fn all_sessions(&self) -> SessionDbResult<Vec<Session>> {
        self.store.with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT id FROM sessions")
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
            let mut sessions = Vec::new();
            for row in rows {
                let id = row.map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
                if let Some(session) = load_session(conn, &id)? {
                    sessions.push(session);
                }
            }
            Ok(sessions)
        })
    }

    pub fn touch(&self, id: &str) -> SessionDbResult<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.store.with_connection(|conn| {
            conn.execute(
                "UPDATE sessions SET last_active = ?1 WHERE id = ?2",
                params![now, id],
            )
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
            Ok(())
        })
    }
}

pub(crate) fn insert_session(conn: &Connection, session: &Session) -> SessionDbResult<()> {
    let permissions_json = serde_json::to_string(&session.permissions)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let state_json = serde_json::to_string(&session.state)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let channel_type_json = serde_json::to_string(&session.channel_type)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let created_at = session.created_at.to_rfc3339();
    let last_active = session.last_active.to_rfc3339();

    let tx = conn
        .unchecked_transaction()
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    tx.execute(
        "INSERT OR REPLACE INTO sessions
         (id, channel_type, channel_id, user_id, permissions_json, created_at, last_active, state_json, summary)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            session.id,
            channel_type_json,
            session.channel_id,
            session.user_id,
            permissions_json,
            created_at,
            last_active,
            state_json,
            Option::<String>::None,
        ],
    )
    .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;

    tx.execute(
        "DELETE FROM messages WHERE session_id = ?1",
        params![session.id],
    )
    .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;

    let now = chrono::Utc::now().to_rfc3339();
    for (idx, message) in session.conversation.iter().enumerate() {
        let (message_type, content, tool_call_id) = match message {
            Message::System { content } => ("system", content, None),
            Message::User { content } => ("user", content, None),
            Message::Assistant { content } => ("assistant", content, None),
            Message::Tool {
                tool_call_id,
                content,
            } => ("tool", content, Some(tool_call_id)),
        };
        tx.execute(
            "INSERT INTO messages
             (session_id, message_type, content, tool_call_id, created_at, seq_order, token_estimate)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                session.id,
                message_type,
                content,
                tool_call_id,
                now,
                idx as i64,
                Option::<i64>::None,
            ],
        )
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    }

    tx.commit()
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    Ok(())
}

pub(crate) fn load_session(conn: &Connection, id: &str) -> SessionDbResult<Option<Session>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, channel_type, channel_id, user_id, permissions_json, created_at, last_active, state_json
             FROM sessions WHERE id = ?1",
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

    let channel_type_json: String = row
        .get(1)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let channel_type: ChannelType = serde_json::from_str(&channel_type_json)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let permissions_json: String = row
        .get(4)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let permissions: CapabilitySet = serde_json::from_str(&permissions_json)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let created_at = parse_datetime(
        row.get::<_, String>(5)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
    )?;
    let last_active = parse_datetime(
        row.get::<_, String>(6)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
    )?;
    let state_json: String = row
        .get(7)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let state: SessionState = serde_json::from_str(&state_json)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;

    let mut message_stmt = conn
        .prepare(
            "SELECT message_type, content, tool_call_id
             FROM messages WHERE session_id = ?1 ORDER BY seq_order",
        )
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let message_rows = message_stmt
        .query_map(params![id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let mut conversation = Vec::new();
    for row in message_rows {
        let (message_type, content, tool_call_id) =
            row.map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
        conversation.push(message_from_row(&message_type, content, tool_call_id)?);
    }

    Ok(Some(Session {
        id: row
            .get(0)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        channel_type,
        channel_id: row
            .get(2)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        user_id: row
            .get(3)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        conversation,
        permissions,
        created_at,
        last_active,
        state,
    }))
}

fn message_from_row(
    message_type: &str,
    content: String,
    tool_call_id: Option<String>,
) -> SessionDbResult<Message> {
    match message_type {
        "system" => Ok(Message::system(content)),
        "user" => Ok(Message::user(content)),
        "assistant" => Ok(Message::assistant(content)),
        "tool" => Ok(Message::tool(
            tool_call_id.unwrap_or_else(|| "unknown".to_string()),
            content,
        )),
        other => Err(SessionDbError::QueryFailed(format!(
            "unknown message_type '{other}'",
        ))),
    }
}

fn parse_datetime(value: String) -> SessionDbResult<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(&value)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))
}
