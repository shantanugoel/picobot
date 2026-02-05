use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;

use crate::session::db::SqliteStore;
use crate::session::error::{SessionDbError, SessionDbResult};

pub fn spawn_retention_task(
    store: SqliteStore,
    max_age_days: u32,
    interval_secs: u64,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            let _ = purge_old_messages(&store, max_age_days);
        }
    })
}

pub fn spawn_summarization_task(
    store: SqliteStore,
    model: Arc<dyn crate::models::traits::Model>,
    trigger_tokens: u32,
    interval_secs: u64,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            let sessions = match store.with_connection(|conn| {
                let mut stmt = conn
                    .prepare("SELECT DISTINCT session_id FROM messages")
                    .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
                let mut ids = Vec::new();
                for row in rows {
                    ids.push(row.map_err(|err| SessionDbError::QueryFailed(err.to_string()))?);
                }
                Ok(ids)
            }) {
                Ok(ids) => ids,
                Err(_) => continue,
            };
            for session_id in sessions {
                if should_summarize(&store, &session_id, trigger_tokens) {
                    let message_count = count_messages(&store, &session_id).unwrap_or(0);
                    let store_clone = store.clone();
                    let model_clone = Arc::clone(&model);
                    tokio::spawn(async move {
                        let _ = crate::session::summarization::summarize_session(
                            &store_clone,
                            model_clone.as_ref(),
                            &session_id,
                            message_count,
                        )
                        .await;
                    });
                }
            }
        }
    })
}

pub fn purge_old_messages(store: &SqliteStore, max_age_days: u32) -> SessionDbResult<()> {
    if max_age_days == 0 {
        return Ok(());
    }
    let cutoff = chrono::Utc::now()
        .checked_sub_signed(chrono::Duration::days(max_age_days as i64))
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
    store.with_connection(|conn| {
        conn.execute("DELETE FROM messages WHERE created_at < ?1", [cutoff])
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
        Ok(())
    })
}

fn should_summarize(store: &SqliteStore, session_id: &str, trigger_tokens: u32) -> bool {
    let threshold = trigger_tokens.max(1) as usize;
    let count = count_messages(store, session_id).unwrap_or(0);
    count >= threshold
}

fn count_messages(store: &SqliteStore, session_id: &str) -> SessionDbResult<usize> {
    store.with_connection(|conn| {
        let mut stmt = conn
            .prepare("SELECT COUNT(*) FROM messages WHERE session_id = ?1")
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
        let count: i64 = stmt
            .query_row([session_id], |row| row.get(0))
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
        Ok(count as usize)
    })
}
