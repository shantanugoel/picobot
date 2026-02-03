use std::sync::Arc;

use crate::channels::adapter::ChannelType;
use crate::channels::permissions::ChannelPermissionProfile;
use crate::config::ServerConfig;
use crate::delivery::tracking::DeliveryTracker;
use crate::kernel::agent::Kernel;
use crate::models::router::ModelRegistry;
use crate::scheduler::service::SchedulerService;
use crate::server::rate_limit::RateLimiter;
use crate::session::persistent_manager::PersistentSessionManager;
use crate::session::retention::{spawn_retention_task, spawn_summarization_task};
use tokio::sync::broadcast;
use tokio::sync::watch;

#[derive(Clone)]
pub struct AppState {
    pub kernel: Arc<Kernel>,
    pub models: Arc<ModelRegistry>,
    pub sessions: Arc<PersistentSessionManager>,
    pub deliveries: DeliveryTracker,
    pub api_profile: ChannelPermissionProfile,
    pub websocket_profile: ChannelPermissionProfile,
    pub server_config: Option<ServerConfig>,
    pub rate_limiter: Option<RateLimiter>,
    pub snapshot_path: Option<String>,
    pub max_tool_rounds: usize,
    pub channel_type: ChannelType,
    pub whatsapp_qr: Option<broadcast::Sender<String>>,
    pub whatsapp_qr_cache: Option<watch::Receiver<Option<String>>>,
    pub scheduler: Option<Arc<SchedulerService>>,
}

pub fn maybe_start_retention(config: &crate::config::Config, models: &Arc<ModelRegistry>) {
    let retention = config
        .session
        .as_ref()
        .and_then(|session| session.retention.clone())
        .unwrap_or_default();
    let memory = config
        .session
        .as_ref()
        .and_then(|session| session.memory.clone())
        .unwrap_or_default();
    let max_age_days = retention.max_age_days.unwrap_or(90);
    let interval_secs = retention.cleanup_interval_secs.unwrap_or(3600);
    let data_dir = config.data.as_ref().and_then(|data| data.dir.as_deref());
    let store_path = std::path::PathBuf::from(data_dir.unwrap_or("data"))
        .join("conversations.db")
        .to_string_lossy()
        .to_string();
    let store = crate::session::db::SqliteStore::new(store_path);
    let _ = store.touch();
    let _task = spawn_retention_task(store.clone(), max_age_days, interval_secs);
    if memory.enable_summarization.unwrap_or(true) {
        let trigger = memory.summarization_trigger_tokens.unwrap_or(8000);
        let model = models.default_model_arc();
        let _task = spawn_summarization_task(store, model, trigger, interval_secs);
    }
}
