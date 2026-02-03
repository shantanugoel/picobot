use std::fs;
use std::path::Path;

use crate::session::db::SqliteStore;
use crate::session::error::SessionDbResult;
use crate::session::persistent_manager::insert_session;
use crate::session::snapshot::SessionSnapshot;

pub fn migrate_snapshot_if_present(
    store: &SqliteStore,
    snapshot_path: &str,
) -> SessionDbResult<()> {
    let path = Path::new(snapshot_path);
    if !path.exists() {
        return Ok(());
    }
    let raw = fs::read_to_string(path)
        .map_err(|err| crate::session::error::SessionDbError::MigrationFailed(err.to_string()))?;
    let snapshot: SessionSnapshot = serde_json::from_str(&raw)
        .map_err(|err| crate::session::error::SessionDbError::MigrationFailed(err.to_string()))?;
    store.with_connection(|conn| {
        for session in &snapshot.sessions {
            insert_session(conn, session)?;
        }
        Ok(())
    })?;
    let migrated = path.with_extension("json.migrated");
    fs::rename(path, migrated)
        .map_err(|err| crate::session::error::SessionDbError::MigrationFailed(err.to_string()))?;
    Ok(())
}
