use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::session::manager::Session;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub sessions: Vec<Session>,
}

#[derive(Debug, Clone)]
pub struct SnapshotStore {
    path: Arc<String>,
}

impl SnapshotStore {
    pub fn new(path: String) -> Self {
        Self {
            path: Arc::new(path),
        }
    }

    pub fn load(&self) -> Result<Option<SessionSnapshot>, String> {
        let path = Path::new(self.path.as_str());
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(path).map_err(|err| err.to_string())?;
        let snapshot = serde_json::from_str(&raw).map_err(|err| err.to_string())?;
        Ok(Some(snapshot))
    }

    pub fn save(&self, sessions: &[Session]) -> Result<(), String> {
        let snapshot = SessionSnapshot {
            sessions: sessions.to_vec(),
        };
        let raw = serde_json::to_string_pretty(&snapshot).map_err(|err| err.to_string())?;
        if let Some(parent) = Path::new(self.path.as_str()).parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        fs::write(self.path.as_str(), raw).map_err(|err| err.to_string())?;
        Ok(())
    }
}
