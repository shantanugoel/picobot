use std::sync::Arc;

use crate::channels::adapter::ChannelType;
use crate::channels::permissions::ChannelPermissionProfile;
use crate::config::ServerConfig;
use crate::kernel::agent::Kernel;
use crate::models::router::ModelRegistry;
use crate::server::rate_limit::RateLimiter;
use crate::session::manager::SessionManager;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct AppState {
    pub kernel: Arc<Kernel>,
    pub models: Arc<ModelRegistry>,
    pub sessions: Arc<SessionManager>,
    pub api_profile: ChannelPermissionProfile,
    pub websocket_profile: ChannelPermissionProfile,
    pub server_config: Option<ServerConfig>,
    pub rate_limiter: Option<RateLimiter>,
    pub snapshot_path: Option<String>,
    pub max_tool_rounds: usize,
    pub channel_type: ChannelType,
    pub whatsapp_qr: Option<broadcast::Sender<String>>,
}
