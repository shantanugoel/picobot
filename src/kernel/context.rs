use std::path::PathBuf;
use std::sync::Arc;

use crate::kernel::permissions::CapabilitySet;

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub working_dir: PathBuf,
    pub capabilities: Arc<CapabilitySet>,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
}
