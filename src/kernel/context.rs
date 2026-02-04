use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::kernel::permissions::CapabilitySet;

#[derive(Clone)]
pub struct ToolContext {
    pub working_dir: PathBuf,
    pub capabilities: Arc<CapabilitySet>,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub scheduler: Arc<RwLock<Option<Arc<crate::scheduler::service::SchedulerService>>>>,
    pub log_model_requests: bool,
}

impl ToolContext {
    pub fn scheduler(&self) -> Option<Arc<crate::scheduler::service::SchedulerService>> {
        self.scheduler
            .read()
            .ok()
            .and_then(|slot| slot.as_ref().cloned())
    }
}
