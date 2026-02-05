use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;

use crate::session::persistent_manager::PersistentSessionManager;
use crate::session::snapshot::SnapshotStore;

pub fn spawn_snapshot_task(
    sessions: Arc<PersistentSessionManager>,
    store: SnapshotStore,
    interval_secs: u64,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            if let Ok(sessions) = sessions.all_sessions() {
                let _ = store.save(&sessions);
            }
        }
    })
}
