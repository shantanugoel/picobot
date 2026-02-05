use std::fs;
use std::path::Path;
use std::sync::Arc;

use rusqlite::{Connection, OpenFlags, params};

use crate::session::error::{SessionDbError, SessionDbResult};

#[derive(Debug, Clone)]
pub struct SqliteStore {
    path: Arc<String>,
}

impl SqliteStore {
    pub fn new(path: String) -> Self {
        Self {
            path: Arc::new(path),
        }
    }

    pub fn path(&self) -> &str {
        self.path.as_str()
    }

    pub fn ensure_parent_dir(&self) -> SessionDbResult<()> {
        if let Some(parent) = Path::new(self.path.as_str()).parent() {
            fs::create_dir_all(parent)
                .map_err(|err| SessionDbError::OpenFailed(err.to_string()))?;
        }
        Ok(())
    }

    pub fn open(&self) -> SessionDbResult<Connection> {
        self.ensure_parent_dir()?;
        Connection::open_with_flags(
            self.path.as_str(),
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
        )
        .map_err(|err| SessionDbError::OpenFailed(err.to_string()))
    }

    pub fn migrate(&self, conn: &Connection) -> SessionDbResult<()> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                channel_type TEXT NOT NULL,
                channel_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                permissions_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                last_active TEXT NOT NULL,
                state_json TEXT NOT NULL,
                summary TEXT
            );
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                message_type TEXT NOT NULL CHECK(message_type IN ('system', 'user', 'assistant', 'assistant_tool_calls', 'tool')),
                content TEXT NOT NULL,
                tool_call_id TEXT,
                created_at TEXT NOT NULL,
                seq_order INTEGER NOT NULL,
                token_estimate INTEGER
            );
            CREATE TABLE IF NOT EXISTS user_memories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id TEXT NOT NULL,
                key TEXT NOT NULL,
                content TEXT NOT NULL,
                source_session_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE(user_id, key)
            );
            CREATE TABLE IF NOT EXISTS session_summaries (
                session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                summary TEXT NOT NULL,
                message_count INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_messages_session_order ON messages(session_id, seq_order);
            CREATE INDEX IF NOT EXISTS idx_messages_created ON messages(created_at);
            CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);
            CREATE INDEX IF NOT EXISTS idx_sessions_channel ON sessions(channel_id);
            CREATE INDEX IF NOT EXISTS idx_user_memories_user ON user_memories(user_id);
            CREATE INDEX IF NOT EXISTS idx_session_summaries_session ON session_summaries(session_id);
            CREATE TABLE IF NOT EXISTS schedules (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                schedule_type TEXT NOT NULL CHECK(schedule_type IN ('interval', 'once', 'cron')),
                schedule_expr TEXT NOT NULL,
                task_prompt TEXT NOT NULL,
                session_id TEXT,
                user_id TEXT NOT NULL,
                channel_id TEXT,
                capabilities_json TEXT NOT NULL,
                creator_principal TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                max_executions INTEGER,
                execution_count INTEGER NOT NULL DEFAULT 0,
                created_by_system INTEGER NOT NULL DEFAULT 0,
                claimed_at TEXT,
                claim_id TEXT,
                claim_expires_at TEXT,
                last_run_at TEXT,
                next_run_at TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                consecutive_failures INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                backoff_until TEXT,
                metadata_json TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_schedules_due ON schedules(next_run_at, enabled, claimed_at);
            CREATE INDEX IF NOT EXISTS idx_schedules_user ON schedules(user_id);
            CREATE INDEX IF NOT EXISTS idx_schedules_claim ON schedules(claim_id);
            CREATE TABLE IF NOT EXISTS schedule_executions (
                id TEXT PRIMARY KEY,
                job_id TEXT NOT NULL REFERENCES schedules(id) ON DELETE CASCADE,
                started_at TEXT NOT NULL,
                completed_at TEXT,
                status TEXT NOT NULL CHECK(status IN ('running', 'completed', 'failed', 'timeout', 'cancelled')),
                result_summary TEXT,
                error TEXT,
                execution_time_ms INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_schedule_executions_job ON schedule_executions(job_id, started_at);",
        )
        .map_err(|err| SessionDbError::MigrationFailed(err.to_string()))?;
        if let Err(err) = conn.execute(
            "ALTER TABLE schedules ADD COLUMN created_by_system INTEGER NOT NULL DEFAULT 0",
            [],
        ) && !err.to_string().contains("duplicate column")
        {
            return Err(SessionDbError::MigrationFailed(err.to_string()));
        }
        Ok(())
    }

    pub fn touch(&self) -> SessionDbResult<()> {
        let conn = self.open()?;
        self.migrate(&conn)?;
        Ok(())
    }

    pub fn with_connection<F, T>(&self, f: F) -> SessionDbResult<T>
    where
        F: FnOnce(&Connection) -> SessionDbResult<T>,
    {
        let conn = self.open()?;
        self.migrate(&conn)?;
        f(&conn)
    }

    pub fn insert_probe(&self, conn: &Connection) -> SessionDbResult<()> {
        conn.execute(
            "INSERT INTO sessions (id, channel_type, channel_id, user_id, permissions_json, created_at, last_active, state_json, summary)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                "probe",
                "api",
                "probe",
                "probe",
                "{}",
                chrono::Utc::now().to_rfc3339(),
                chrono::Utc::now().to_rfc3339(),
                "{}",
                Option::<String>::None
            ],
        )
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
        conn.execute("DELETE FROM sessions WHERE id = ?1", params!["probe"])
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::SqliteStore;
    use uuid::Uuid;

    #[test]
    fn sqlite_store_creates_schema() {
        let dir = std::env::temp_dir().join(format!("picobot-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sessions.db");
        let store = SqliteStore::new(path.to_string_lossy().to_string());
        let conn = store.open().unwrap();
        store.migrate(&conn).unwrap();
        store.insert_probe(&conn).unwrap();
        fs::remove_dir_all(&dir).ok();
    }
}
