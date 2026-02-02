use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
pub struct Config {
    pub agent: Option<AgentConfig>,
    pub permissions: Option<PermissionsConfig>,
}

#[derive(Debug, Deserialize)]
pub struct AgentConfig {
    pub name: Option<String>,
    pub system_prompt: Option<String>,
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
