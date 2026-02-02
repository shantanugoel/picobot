use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
pub struct Config {
    pub agent: Option<AgentConfig>,
    #[serde(default)]
    pub models: Vec<ModelConfig>,
    #[serde(default)]
    pub routing: Option<RoutingConfig>,
    pub permissions: Option<PermissionsConfig>,
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
