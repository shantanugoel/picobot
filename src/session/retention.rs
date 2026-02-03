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
