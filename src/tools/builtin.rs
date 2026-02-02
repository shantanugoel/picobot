use crate::config::PermissionsConfig;
use crate::tools::filesystem::FilesystemTool;
use crate::tools::http::HttpTool;
use crate::tools::registry::{ToolRegistry, ToolRegistryError};
use crate::tools::shell::ShellTool;
use crate::tools::traits::ToolError;

pub fn register_builtin_tools(
    _permissions: Option<&PermissionsConfig>,
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

    Ok(registry)
}
