use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::kernel::permissions::{PathPattern, Permission};
use crate::tools::traits::{Tool, ToolContext, ToolError, ToolOutput};

#[derive(Debug, Default)]
pub struct FilesystemTool;

#[async_trait]
impl Tool for FilesystemTool {
    fn name(&self) -> &'static str {
        "filesystem"
    }

    fn description(&self) -> &'static str {
        "Read or write local files"
    }

    fn schema(&self) -> Value {
        json!({
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
        })
    }

    fn required_permissions(
        &self,
        _ctx: &ToolContext,
        input: &Value,
    ) -> Result<Vec<Permission>, ToolError> {
        let operation = input
            .get("operation")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("missing operation".to_string()))?;
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("missing path".to_string()))?;

        let resolved = resolve_path(&_ctx.working_dir, path)?;
        let canonical = if resolved.exists() {
            resolved
                .canonicalize()
                .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?
        } else {
            resolved.clone()
        };
        let pattern = PathPattern(canonical.to_string_lossy().to_string());
        let permission = match operation {
            "read" => Permission::FileRead { path: pattern },
            "write" => Permission::FileWrite { path: pattern },
            _ => return Err(ToolError::InvalidInput("invalid operation".to_string())),
        };
        Ok(vec![permission])
    }

    async fn execute(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        let operation = input
            .get("operation")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("missing operation".to_string()))?;
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("missing path".to_string()))?;

        let resolved = resolve_path(&ctx.working_dir, path)?;
        match operation {
            "read" => {
                let data = tokio::fs::read_to_string(&resolved)
                    .await
                    .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;
                Ok(json!({"content": data}))
            }
            "write" => {
                let content = input
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::InvalidInput("missing content".to_string()))?;
                if let Some(parent) = resolved.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;
                }
                tokio::fs::write(&resolved, content)
                    .await
                    .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;
                Ok(json!({"status": "ok"}))
            }
            _ => Err(ToolError::InvalidInput("invalid operation".to_string())),
        }
    }
}

fn resolve_path(working_dir: &Path, raw: &str) -> Result<PathBuf, ToolError> {
    let expanded = if raw.starts_with("~") {
        if raw == "~" || raw.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                let trimmed = raw.trim_start_matches("~");
                home.join(trimmed.trim_start_matches('/'))
            } else {
                return Err(ToolError::ExecutionFailed(
                    "home directory not found".to_string(),
                ));
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

    Ok(normalize_path(&resolved))
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
    use super::normalize_path;

    #[test]
    fn normalize_path_removes_dot_segments() {
        let input = std::path::Path::new("/tmp/a/./b/../c");
        let normalized = normalize_path(input);
        assert_eq!(normalized.to_string_lossy(), "/tmp/a/c");
    }
}
