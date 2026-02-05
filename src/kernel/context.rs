use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::kernel::permissions::CapabilitySet;

#[derive(Clone)]
pub struct ToolContext {
    pub working_dir: PathBuf,
    pub capabilities: Arc<CapabilitySet>,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub channel_id: Option<String>,
    pub scheduler: Arc<RwLock<Option<Arc<crate::scheduler::service::SchedulerService>>>>,
    pub log_model_requests: bool,
    pub include_tool_messages: bool,
    pub host_os: String,
    pub timezone_offset: String,
    pub timezone_name: String,
    pub allowed_shell_commands: Vec<String>,
    pub notifications: Arc<RwLock<Option<Arc<crate::notifications::service::NotificationService>>>>,
    pub scheduled_job: bool,
}

impl ToolContext {
    pub fn scheduler(&self) -> Option<Arc<crate::scheduler::service::SchedulerService>> {
        self.scheduler
            .read()
            .ok()
            .and_then(|slot| slot.as_ref().cloned())
    }

    pub fn notifications(&self) -> Option<Arc<crate::notifications::service::NotificationService>> {
        self.notifications
            .read()
            .ok()
            .and_then(|slot| slot.as_ref().cloned())
    }
}
