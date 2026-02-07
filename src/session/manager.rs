use rusqlite::{params, Connection};

use crate::kernel::permissions::CapabilitySet;
use crate::session::db::SqliteStore;
use crate::session::error::{SessionDbError, SessionDbResult};
use crate::session::types::{MessageType, Session, SessionState, StoredMessage, UsageEvent};

#[derive(Debug, Clone)]
pub struct SessionManager {
    store: SqliteStore,
}

impl SessionManager {
    pub fn new(store: SqliteStore) -> Self {
        Self { store }
    }

    #[allow(dead_code)]
    pub fn store(&self) -> &SqliteStore {
        &self.store
    }

    pub fn create_session(
        &self,
        id: String,
        channel_type: String,
        channel_id: String,
        user_id: String,
        permissions: CapabilitySet,
    ) -> SessionDbResult<Session> {
        let now = chrono::Utc::now();
        let session = Session {
            id: id.clone(),
            channel_type,
            channel_id,
            user_id,
            permissions,
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

    pub fn append_message(&self, session_id: &str, message: &StoredMessage) -> SessionDbResult<()> {
        self.store
            .with_connection(|conn| insert_message(conn, session_id, message))
    }

    pub fn get_messages(
        &self,
        session_id: &str,
        limit: usize,
    ) -> SessionDbResult<Vec<StoredMessage>> {
        self.store
            .with_connection(|conn| load_messages(conn, session_id, limit))
    }

    pub fn record_usage(&self, event: &UsageEvent) -> SessionDbResult<()> {
        self.store
            .with_connection(|conn| insert_usage_event(conn, event))
    }
}

fn insert_session(conn: &Connection, session: &Session) -> SessionDbResult<()> {
    let permissions_json = serde_json::to_string(&session.permissions)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let state_json = serde_json::to_string(&session.state)
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let created_at = session.created_at.to_rfc3339();
    let last_active = session.last_active.to_rfc3339();

    conn.execute(
        "INSERT OR REPLACE INTO sessions
         (id, channel_type, channel_id, user_id, permissions_json, created_at, last_active, state_json, summary)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            session.id,
            session.channel_type,
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
    Ok(())
}

fn load_session(conn: &Connection, id: &str) -> SessionDbResult<Option<Session>> {
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

    Ok(Some(Session {
        id: row
            .get(0)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        channel_type: row
            .get(1)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        channel_id: row
            .get(2)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        user_id: row
            .get(3)
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?,
        permissions,
        created_at,
        last_active,
        state,
    }))
}

fn insert_message(
    conn: &Connection,
    session_id: &str,
    message: &StoredMessage,
) -> SessionDbResult<()> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO messages
         (session_id, message_type, content, tool_call_id, created_at, seq_order, token_estimate)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            session_id,
            message.message_type.as_str(),
            message.content,
            message.tool_call_id,
            now,
            message.seq_order,
            message.token_estimate,
        ],
    )
    .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    Ok(())
}

fn load_messages(
    conn: &Connection,
    session_id: &str,
    limit: usize,
) -> SessionDbResult<Vec<StoredMessage>> {
    let mut stmt = conn
        .prepare(
            "SELECT message_type, content, tool_call_id, seq_order, token_estimate
             FROM messages WHERE session_id = ?1 ORDER BY seq_order DESC LIMIT ?2",
        )
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let rows = stmt
        .query_map(params![session_id, limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let mut messages = Vec::new();
    for row in rows {
        let (message_type, content, tool_call_id, seq_order, token_estimate) =
            row.map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
        let message_type = MessageType::parse(&message_type)
            .ok_or_else(|| SessionDbError::QueryFailed("unknown message_type".to_string()))?;
        messages.push(StoredMessage {
            message_type,
            content,
            tool_call_id,
            seq_order,
            token_estimate,
        });
    }
    messages.sort_by_key(|message| message.seq_order);
    Ok(messages)
}

fn insert_usage_event(conn: &Connection, event: &UsageEvent) -> SessionDbResult<()> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO usage_events
         (session_id, channel_id, user_id, provider, model, input_tokens, output_tokens, total_tokens, cached_input_tokens, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            event.session_id,
            event.channel_id,
            event.user_id,
            event.provider,
            event.model,
            event.input_tokens as i64,
            event.output_tokens as i64,
            event.total_tokens as i64,
            event.cached_input_tokens as i64,
            now,
        ],
    )
    .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    Ok(())
}

fn parse_datetime(value: String) -> SessionDbResult<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(&value)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))
}
