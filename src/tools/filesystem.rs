use async_trait::async_trait;
use serde_json::{Value, json};

use crate::kernel::permissions::{PathPattern, Permission};
use crate::tools::path_utils::resolve_path;
use crate::tools::traits::{ToolContext, ToolError, ToolExecutor, ToolOutput, ToolSpec};

#[derive(Debug, Default)]
pub struct FilesystemTool {
    spec: ToolSpec,
}

impl FilesystemTool {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "filesystem".to_string(),
                description: "Read or write files within the allowed directory. path is relative to the working directory; jail escapes are rejected. write requires content. Returns {content} on read, {status} on write."
                    .to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["operation", "path"],
                    "properties": {
                        "operation": {
                            "type": "string",
                            "enum": ["read", "write"]
                        },
                        "path": {
                            "type": "string",
                            "minLength": 1
                        },
                        "content": {
                            "type": "string"
                        }
                    },
                    "additionalProperties": false
                }),
            },
        }
    }
}

#[async_trait]
impl ToolExecutor for FilesystemTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn required_permissions(
        &self,
        ctx: &ToolContext,
        input: &Value,
    ) -> Result<Vec<Permission>, ToolError> {
        let operation = input
            .get("operation")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("missing operation".to_string()))?;
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("missing path".to_string()))?;

        let resolved = resolve_path(&ctx.working_dir, ctx.jail_root.as_deref(), path)?;
        let pattern = PathPattern(resolved.canonical.to_string_lossy().to_string());
        let permission = match operation {
            "read" => Permission::FileRead { path: pattern },
            "write" => Permission::FileWrite { path: pattern },
            _ => return Err(ToolError::new("invalid operation".to_string())),
        };
        Ok(vec![permission])
    }

    async fn execute(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        let operation = input
            .get("operation")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("missing operation".to_string()))?;
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("missing path".to_string()))?;

        let resolved = resolve_path(&ctx.working_dir, ctx.jail_root.as_deref(), path)?;
        match operation {
            "read" => {
                let data = std::fs::read_to_string(&resolved.canonical)
                    .map_err(|err| ToolError::new(err.to_string()))?;
                Ok(json!({"content": data}))
            }
            "write" => {
                let content = input
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::new("missing content".to_string()))?;
                if let Some(parent) = resolved.canonical.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|err| ToolError::new(err.to_string()))?;
                }
                let re_resolved = resolve_path(&ctx.working_dir, ctx.jail_root.as_deref(), path)?;
                if re_resolved.canonical != resolved.canonical {
                    return Err(ToolError::new(
                        "path changed after directory creation".to_string(),
                    ));
                }
                std::fs::write(&re_resolved.canonical, content)
                    .map_err(|err| ToolError::new(err.to_string()))?;
                Ok(json!({"status": "ok"}))
            }
            _ => Err(ToolError::new("invalid operation".to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FilesystemTool;
    use crate::kernel::permissions::{CapabilitySet, Permission};
    use crate::tools::path_utils::{normalize_path, resolve_path};
    use crate::tools::traits::{ExecutionMode, ToolContext, ToolExecutor};
    use serde_json::json;

    #[test]
    fn normalize_path_removes_dot_segments() {
        let input = std::path::Path::new("/tmp/a/./b/../c");
        let normalized = normalize_path(input);
        assert_eq!(normalized.to_string_lossy(), "/tmp/a/c");
    }

    #[test]
    fn resolve_path_expands_relative() {
        let resolved = resolve_path(std::path::Path::new("/tmp"), None, "nested/file.txt").unwrap();
        assert!(
            resolved
                .canonical
                .ends_with(std::path::Path::new("nested/file.txt"))
        );
    }

    #[test]
    fn resolve_path_enforces_jail_root() {
        let jail_root = std::env::temp_dir().join(format!("picobot-jail-{}", uuid::Uuid::new_v4()));
        let inside = jail_root.join("inside");
        let outside =
            std::env::temp_dir().join(format!("picobot-outside-{}", uuid::Uuid::new_v4()));

        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        let jail_root = jail_root.canonicalize().unwrap();
        let allowed = resolve_path(&inside, Some(&jail_root), "file.txt").unwrap();
        let allowed_parent = allowed.canonical.parent().unwrap().canonicalize().unwrap();
        assert!(allowed_parent.starts_with(&jail_root));

        let denied = resolve_path(
            &inside,
            Some(&jail_root),
            outside.to_string_lossy().as_ref(),
        );
        assert!(denied.is_err());

        let _ = std::fs::remove_dir_all(&jail_root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn required_permissions_match_operation() {
        let tool = FilesystemTool::new();
        let ctx = ToolContext {
            working_dir: std::path::PathBuf::from("/tmp"),
            capabilities: std::sync::Arc::new(CapabilitySet::empty()),
            user_id: None,
            session_id: None,
            channel_id: None,
            jail_root: None,
            scheduler: None,
            notifications: None,
            notify_tool_used: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            execution_mode: ExecutionMode::User,
            timezone_offset: "+00:00".to_string(),
            timezone_name: "UTC".to_string(),
            max_response_bytes: None,
        };

        let read = tool
            .required_permissions(&ctx, &json!({"operation": "read", "path": "file.txt"}))
            .unwrap();
        assert!(matches!(read[0], Permission::FileRead { .. }));

        let write = tool
            .required_permissions(&ctx, &json!({"operation": "write", "path": "file.txt"}))
            .unwrap();
        assert!(matches!(write[0], Permission::FileWrite { .. }));
    }
}
