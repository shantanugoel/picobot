use std::path::{Path, PathBuf};

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use serde_json::{Value, json};

use rig::completion::message::{ImageDetail, ImageMediaType, Message, MimeType, UserContent};
use rig::OneOrMany;

use crate::kernel::permissions::{PathPattern, Permission};
use crate::providers::factory::{DEFAULT_PROVIDER_RETRIES, ProviderAgent};
use crate::tools::traits::{ToolContext, ToolError, ToolExecutor, ToolOutput, ToolSpec};

pub struct VisionTool {
    spec: ToolSpec,
    agent: ProviderAgent,
    max_image_size_bytes: u64,
}

impl VisionTool {
    pub fn new(agent: ProviderAgent, max_image_size_bytes: u64) -> Self {
        Self {
            spec: ToolSpec {
                name: "vision".to_string(),
                description: "Analyze an image file using a vision model. Required: path. Optional: question, mime_type, detail (low/high/auto)."
                    .to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["path"],
                    "properties": {
                        "path": { "type": "string" },
                        "question": { "type": "string" },
                        "mime_type": { "type": "string" },
                        "detail": { "type": "string", "enum": ["low", "high", "auto"] }
                    },
                    "additionalProperties": false
                }),
            },
            agent,
            max_image_size_bytes,
        }
    }
}

#[async_trait]
impl ToolExecutor for VisionTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn required_permissions(
        &self,
        ctx: &ToolContext,
        input: &Value,
    ) -> Result<Vec<Permission>, ToolError> {
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
        Ok(vec![Permission::FileRead { path: pattern }])
    }

    async fn execute(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("missing path".to_string()))?;
        let question = input
            .get("question")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("Describe the image.");
        let detail = input
            .get("detail")
            .and_then(Value::as_str)
            .map(parse_detail)
            .transpose()?;
        let mime_override = input.get("mime_type").and_then(Value::as_str);

        let resolved = resolve_path(&ctx.working_dir, ctx.jail_root.as_deref(), path)?;
        let size = std::fs::metadata(&resolved)
            .map_err(|err| ToolError::new(err.to_string()))?
            .len();
        if size > self.max_image_size_bytes {
            return Err(ToolError::new(format!(
                "image is too large: {size} bytes (limit {})",
                self.max_image_size_bytes
            )));
        }
        let bytes = std::fs::read(&resolved).map_err(|err| ToolError::new(err.to_string()))?;
        if bytes.len() as u64 > self.max_image_size_bytes {
            return Err(ToolError::new(format!(
                "image is too large: {} bytes (limit {})",
                bytes.len(),
                self.max_image_size_bytes
            )));
        }

        let mime_type = mime_override
            .map(|value| value.to_string())
            .or_else(|| infer_mime_type(&resolved))
            .ok_or_else(|| ToolError::new("missing or unsupported image mime_type".to_string()))?;
        let media_type = ImageMediaType::from_mime_type(&mime_type)
            .ok_or_else(|| ToolError::new("unsupported image mime_type".to_string()))?;
        let detail = detail.unwrap_or(ImageDetail::Auto);

        let encoded = BASE64_STANDARD.encode(bytes);
        let content = OneOrMany::many(vec![
            UserContent::text(question),
            UserContent::image_base64(encoded, Some(media_type), Some(detail)),
        ])
        .map_err(|_| ToolError::new("failed to build vision prompt".to_string()))?;
        let message = Message::User { content };

        let response = self
            .agent
            .prompt_message_with_retry(message, DEFAULT_PROVIDER_RETRIES)
            .await
            .map_err(|err| ToolError::new(err.to_string()))?;

        Ok(json!({
            "description": response,
            "mime_type": mime_type,
            "bytes": size
        }))
    }
}

fn parse_detail(value: &str) -> Result<ImageDetail, ToolError> {
    match value {
        "low" => Ok(ImageDetail::Low),
        "high" => Ok(ImageDetail::High),
        "auto" => Ok(ImageDetail::Auto),
        _ => Err(ToolError::new("invalid detail".to_string())),
    }
}

fn infer_mime_type(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let mime = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "svg" | "svgz" => "image/svg+xml",
        _ => return None,
    };
    Some(mime.to_string())
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
