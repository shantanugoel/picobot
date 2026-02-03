use crate::config::PermissionsConfig;
use crate::kernel::permissions::Permission;
use crate::session::db::SqliteStore;
use crate::tools::filesystem::FilesystemTool;
use crate::tools::http::HttpTool;
use crate::tools::memory::MemoryTool;
use crate::tools::registry::{ToolRegistry, ToolRegistryError};
use crate::tools::shell::ShellTool;
use crate::tools::traits::ToolError;

pub fn register_builtin_tools(
    permissions: Option<&PermissionsConfig>,
    data_dir: Option<&str>,
) -> Result<ToolRegistry, ToolRegistryError> {
    let mut registry = ToolRegistry::new();

    registry.register(Box::new(FilesystemTool))?;
    registry.register(Box::new(ShellTool))?;

    let http = HttpTool::new().map_err(|err| match err {
        ToolError::ExecutionFailed(detail) => ToolRegistryError::BuiltinToolInitFailed {
            tool: "http_fetch".to_string(),
            detail,
        },
        _ => ToolRegistryError::BuiltinToolInitFailed {
            tool: "http_fetch".to_string(),
            detail: "failed to initialize".to_string(),
        },
    })?;
    registry.register(Box::new(http))?;

    let base_dir = data_dir.unwrap_or("data");
    let store_path = std::path::PathBuf::from(base_dir).join("conversations.db");
    let store = SqliteStore::new(store_path.to_string_lossy().to_string());
    store.touch().map_err(|err| ToolRegistryError::BuiltinToolInitFailed {
        tool: "memory".to_string(),
        detail: err.to_string(),
    })?;
    registry.register(Box::new(MemoryTool::new(store)))?;

    if let Some(permissions) = permissions
        && let Some(shell) = &permissions.shell
        && !shell.allowed_commands.is_empty()
    {
        let builtin = BuiltinPermissionsTool::new(shell.allowed_commands.clone());
        registry.register(Box::new(builtin))?;
    }

    Ok(registry)
}

#[derive(Debug)]
struct BuiltinPermissionsTool {
    allowed_commands: Vec<String>,
}

impl BuiltinPermissionsTool {
    fn new(allowed_commands: Vec<String>) -> Self {
        Self { allowed_commands }
    }
}

#[async_trait::async_trait]
impl crate::tools::traits::Tool for BuiltinPermissionsTool {
    fn name(&self) -> &'static str {
        "permissions"
    }

    fn description(&self) -> &'static str {
        "Show configured shell allowlist"
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false
        })
    }

    fn required_permissions(
        &self,
        _ctx: &crate::tools::traits::ToolContext,
        _input: &serde_json::Value,
    ) -> Result<Vec<Permission>, crate::tools::traits::ToolError> {
        Ok(vec![])
    }

    async fn execute(
        &self,
        _ctx: &crate::tools::traits::ToolContext,
        _input: serde_json::Value,
    ) -> Result<crate::tools::traits::ToolOutput, crate::tools::traits::ToolError> {
        Ok(serde_json::json!({
            "shell_allowed": self.allowed_commands,
        }))
    }
}
