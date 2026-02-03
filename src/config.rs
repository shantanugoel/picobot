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
    #[serde(default)]
    pub data: Option<DataConfig>,
    #[serde(default)]
    pub scheduler: Option<SchedulerConfig>,
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
    #[serde(default)]
    pub retention: Option<RetentionConfig>,
    #[serde(default)]
    pub memory: Option<MemoryConfig>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct DataConfig {
    pub dir: Option<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct RetentionConfig {
    pub max_age_days: Option<u32>,
    pub cleanup_interval_secs: Option<u64>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct MemoryConfig {
    pub enable_user_memories: Option<bool>,
    pub context_budget_tokens: Option<u32>,
    pub max_session_messages: Option<usize>,
    pub max_user_memories: Option<usize>,
    pub enable_summarization: Option<bool>,
    pub include_summary_on_truncation: Option<bool>,
    pub summarization_trigger_tokens: Option<u32>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct SchedulerConfig {
    pub enabled: Option<bool>,
    pub tick_interval_secs: Option<u64>,
    pub max_concurrent_jobs: Option<usize>,
    pub max_concurrent_per_user: Option<usize>,
    pub max_jobs_per_user: Option<u32>,
    pub max_jobs_per_window: Option<u32>,
    pub window_duration_secs: Option<u64>,
    pub job_timeout_secs: Option<u64>,
    pub max_backoff_secs: Option<u64>,
}

impl SchedulerConfig {
    pub fn enabled(&self) -> bool {
        self.enabled.unwrap_or(false)
    }

    pub fn tick_interval_secs(&self) -> u64 {
        self.tick_interval_secs.unwrap_or(1)
    }

    pub fn max_concurrent_jobs(&self) -> usize {
        self.max_concurrent_jobs.unwrap_or(4)
    }

    pub fn max_concurrent_per_user(&self) -> usize {
        self.max_concurrent_per_user.unwrap_or(2)
    }

    pub fn max_jobs_per_user(&self) -> u32 {
        self.max_jobs_per_user.unwrap_or(50)
    }

    pub fn max_jobs_per_window(&self) -> u32 {
        self.max_jobs_per_window.unwrap_or(100)
    }

    pub fn window_duration_secs(&self) -> u64 {
        self.window_duration_secs.unwrap_or(3600)
    }

    pub fn job_timeout_secs(&self) -> u64 {
        self.job_timeout_secs.unwrap_or(300)
    }

    pub fn max_backoff_secs(&self) -> u64 {
        self.max_backoff_secs.unwrap_or(3600)
    }
}
