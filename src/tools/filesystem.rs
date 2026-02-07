use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::kernel::permissions::{PathPattern, Permission};
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
                description: "Read or write local files. Required: operation (read/write) and path. write requires content."
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
                            "type": "string"
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
        let canonical = if resolved.exists() {
            resolved
                .canonicalize()
                .map_err(|err| ToolError::new(err.to_string()))?
        } else {
            resolved.clone()
        };
        let pattern = PathPattern(canonical.to_string_lossy().to_string());
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
                let data = std::fs::read_to_string(&resolved)
                    .map_err(|err| ToolError::new(err.to_string()))?;
                Ok(json!({"content": data}))
            }
            "write" => {
                let content = input
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::new("missing content".to_string()))?;
                if let Some(parent) = resolved.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|err| ToolError::new(err.to_string()))?;
                }
                std::fs::write(&resolved, content)
                    .map_err(|err| ToolError::new(err.to_string()))?;
                Ok(json!({"status": "ok"}))
            }
            _ => Err(ToolError::new("invalid operation".to_string())),
        }
    }
}

fn resolve_path(
    working_dir: &Path,
    jail_root: Option<&Path>,
    raw: &str,
) -> Result<PathBuf, ToolError> {
    let expanded = if raw.starts_with('~') {
        if raw == "~" || raw.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                let trimmed = raw.trim_start_matches('~');
                home.join(trimmed.trim_start_matches('/'))
            } else {
                return Err(ToolError::new("home directory not found".to_string()));
            }
        } else {
            PathBuf::from(raw)
        }
    } else {
        PathBuf::from(raw)
    };

    let resolved = if expanded.is_absolute() {
        expanded
    } else {
        working_dir.join(expanded)
    };

    let resolved = normalize_path(&resolved);
    if let Some(jail_root) = jail_root {
        let jail_root = jail_root
            .canonicalize()
            .map_err(|err| ToolError::new(format!("invalid jail_root: {err}")))?;
        let candidate = if resolved.exists() {
            resolved
                .canonicalize()
                .map_err(|err| ToolError::new(err.to_string()))?
        } else if let Some(parent) = resolved.parent() {
            let parent = parent
                .canonicalize()
                .map_err(|err| ToolError::new(err.to_string()))?;
            match resolved.file_name() {
                Some(name) => parent.join(name),
                None => parent,
            }
        } else {
            resolved.clone()
        };
        if !candidate.starts_with(&jail_root) {
            return Err(ToolError::new(format!(
                "path escapes jail_root: {}",
                candidate.display()
            )));
        }
    }

    Ok(resolved)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::{FilesystemTool, normalize_path, resolve_path};
    use crate::kernel::permissions::{CapabilitySet, Permission};
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
        assert_eq!(resolved.to_string_lossy(), "/tmp/nested/file.txt");
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
        let allowed_parent = allowed.parent().unwrap().canonicalize().unwrap();
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
