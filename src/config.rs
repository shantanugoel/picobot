use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
pub struct Config {
    pub agent: Option<AgentConfig>,
    #[serde(default)]
    pub models: Vec<ModelConfig>,
    #[serde(default)]
    pub routing: Option<RoutingConfig>,
    pub permissions: Option<PermissionsConfig>,
    #[serde(default)]
    pub logging: Option<LoggingConfig>,
    #[serde(default)]
    pub server: Option<ServerConfig>,
    #[serde(default)]
    pub channels: Option<ChannelsConfig>,
    #[serde(default)]
    pub session: Option<SessionConfig>,
}

#[derive(Debug, Deserialize)]
pub struct AgentConfig {
    pub name: Option<String>,
    pub system_prompt: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct RoutingConfig {
    pub default: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ModelConfig {
    pub id: String,
    pub provider: String,
    pub model: String,
    pub api_key_env: Option<String>,
    pub base_url: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct PermissionsConfig {
    pub filesystem: Option<FilesystemPermissions>,
    pub network: Option<NetworkPermissions>,
    pub shell: Option<ShellPermissions>,
}

#[derive(Debug, Deserialize, Default)]
pub struct FilesystemPermissions {
    pub read_paths: Vec<String>,
    pub write_paths: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct NetworkPermissions {
    pub allowed_domains: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ShellPermissions {
    pub allowed_commands: Vec<String>,
    pub working_directory: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct LoggingConfig {
    pub level: Option<String>,
    pub audit_file: Option<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct ServerConfig {
    pub bind: Option<String>,
    pub expose_externally: Option<bool>,
    #[serde(default)]
    pub auth: Option<AuthConfig>,
    #[serde(default)]
    pub cors: Option<CorsConfig>,
    #[serde(default)]
    pub rate_limit: Option<RateLimitConfig>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct AuthConfig {
    #[serde(default)]
    pub api_keys: Vec<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct CorsConfig {
    #[serde(default)]
    pub allowed_origins: Vec<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct RateLimitConfig {
    pub requests_per_minute: Option<u32>,
    pub per_key: Option<bool>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct ChannelsConfig {
    pub whatsapp: Option<ChannelConfig>,
    pub websocket: Option<ChannelConfig>,
    pub api: Option<ChannelConfig>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct ChannelConfig {
    pub enabled: Option<bool>,
    pub store_path: Option<String>,
    #[serde(default)]
    pub allowed_senders: Vec<String>,
    #[serde(default)]
    pub pre_authorized: Vec<String>,
    #[serde(default)]
    pub max_allowed: Vec<String>,
    pub allow_user_prompts: Option<bool>,
    pub prompt_timeout_secs: Option<u32>,
}

#[derive(Debug, Deserialize, Default)]
pub struct SessionConfig {
    pub snapshot_interval_secs: Option<u64>,
    pub snapshot_path: Option<String>,
}
