use std::sync::Arc;

use serde_json::json;

use picobot::kernel::core::Kernel;
use picobot::kernel::permissions::{CapabilitySet, PathPattern, Permission};
use picobot::tools::filesystem::FilesystemTool;
use picobot::tools::registry::ToolRegistry;

#[tokio::test]
async fn filesystem_read_allowed_via_kernel() {
    let dir = std::env::temp_dir().join(format!("picobot-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("data.txt");
    std::fs::write(&file, "hello").unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(FilesystemTool::new())).unwrap();
    let registry = Arc::new(registry);

    let canonical_dir = dir.canonicalize().unwrap();
    let mut capabilities = CapabilitySet::empty();
    capabilities.insert(Permission::FileRead {
        path: PathPattern(format!("{}/**", canonical_dir.to_string_lossy())),
    });
    let kernel = Kernel::new(Arc::clone(&registry)).with_capabilities(capabilities);

    let tool = kernel.tool_registry().get("filesystem").unwrap();
    let result = kernel
        .invoke_tool(
            tool.as_ref(),
            json!({"operation": "read", "path": file.to_string_lossy()}),
        )
        .await;
    assert!(result.is_ok());

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn filesystem_read_denied_via_kernel() {
    let dir = std::env::temp_dir().join(format!("picobot-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("data.txt");
    std::fs::write(&file, "hello").unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(FilesystemTool::new())).unwrap();
    let registry = Arc::new(registry);

    let kernel = Kernel::new(Arc::clone(&registry));
    let tool = kernel.tool_registry().get("filesystem").unwrap();
    let result = kernel
        .invoke_tool(
            tool.as_ref(),
            json!({"operation": "read", "path": file.to_string_lossy()}),
        )
        .await;
    assert!(result.is_err());

    std::fs::remove_dir_all(&dir).ok();
}
