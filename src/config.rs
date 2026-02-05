use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize, Default, Clone)]
pub struct Config {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub system_prompt: Option<String>,
    pub max_turns: Option<usize>,
    pub bind: Option<String>,
    pub data_dir: Option<String>,
    pub permissions: Option<PermissionsConfig>,
    pub scheduler: Option<SchedulerConfig>,
    pub memory: Option<MemoryConfig>,
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = std::env::var("PICOBOT_CONFIG").unwrap_or_else(|_| "picobot.toml".to_string());
        Self::load_from(PathBuf::from(path))
    }

    pub fn load_from(path: PathBuf) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read config at {}", path.display()))?;
        let config = toml::from_str(&contents)
            .with_context(|| format!("failed to parse config at {}", path.display()))?;
        Ok(config)
    }

    pub fn provider(&self) -> &str {
        self.provider.as_deref().unwrap_or("openai")
    }

    pub fn model(&self) -> &str {
        self.model.as_deref().unwrap_or("gpt-4o-mini")
    }

    pub fn system_prompt(&self) -> &str {
        self.system_prompt
            .as_deref()
            .unwrap_or("You are PicoBot, a helpful assistant.")
    }

    pub fn max_turns(&self) -> usize {
        self.max_turns.unwrap_or(5)
    }

    pub fn bind(&self) -> &str {
        self.bind.as_deref().unwrap_or("127.0.0.1:8080")
    }

    pub fn data_dir(&self) -> PathBuf {
        if let Some(dir) = &self.data_dir {
            return PathBuf::from(dir);
        }
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("picobot")
    }

    pub fn permissions(&self) -> PermissionsConfig {
        self.permissions.clone().unwrap_or_default()
    }

    pub fn scheduler(&self) -> SchedulerConfig {
        self.scheduler.clone().unwrap_or_default()
    }

    pub fn memory(&self) -> MemoryConfig {
        self.memory.clone().unwrap_or_default()
    }
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct PermissionsConfig {
    pub filesystem: Option<FilesystemPermissions>,
    pub network: Option<NetworkPermissions>,
    pub shell: Option<ShellPermissions>,
    pub schedule: Option<SchedulePermissions>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct FilesystemPermissions {
    pub read_paths: Vec<String>,
    pub write_paths: Vec<String>,
    pub jail_root: Option<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct NetworkPermissions {
    pub allowed_domains: Vec<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct ShellPermissions {
    pub allowed_commands: Vec<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct SchedulePermissions {
    pub allowed_actions: Vec<String>,
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

#[derive(Debug, Deserialize, Default, Clone)]
pub struct MemoryConfig {
    pub enable_user_memories: Option<bool>,
    pub context_budget_tokens: Option<u32>,
    pub max_session_messages: Option<usize>,
    pub max_user_memories: Option<usize>,
    pub include_summary_on_truncation: Option<bool>,
    pub include_tool_messages: Option<bool>,
}

impl MemoryConfig {
    pub fn include_tool_messages(&self) -> bool {
        self.include_tool_messages.unwrap_or(true)
    }
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
