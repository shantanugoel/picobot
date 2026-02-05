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
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct PermissionsConfig {
    pub filesystem: Option<FilesystemPermissions>,
    pub network: Option<NetworkPermissions>,
    pub shell: Option<ShellPermissions>,
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
