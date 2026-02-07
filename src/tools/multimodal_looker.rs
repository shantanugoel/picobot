use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use reqwest::Client;
use serde_json::{Value, json};

use rig::OneOrMany;
use rig::completion::message::{
    AudioMediaType, DocumentMediaType, ImageDetail, ImageMediaType, Message, MimeType, UserContent,
    VideoMediaType,
};

use crate::kernel::permissions::{DomainPattern, PathPattern, Permission};
use crate::providers::factory::{DEFAULT_PROVIDER_RETRIES, ProviderAgent};
use crate::tools::net_utils::{ensure_allowed_url, parse_host};
use crate::tools::path_utils::resolve_path;
use crate::tools::traits::{ToolContext, ToolError, ToolExecutor, ToolOutput, ToolSpec};

#[derive(Debug, Clone, Copy)]
enum MediaKind {
    Image,
    Audio,
    Video,
    Document,
}

pub struct MultimodalLookerTool {
    spec: ToolSpec,
    agent: ProviderAgent,
    client: Client,
    max_media_size_bytes: u64,
    max_image_size_bytes: u64,
}

impl MultimodalLookerTool {
    pub fn new(agent: ProviderAgent, max_media_size_bytes: u64, max_image_size_bytes: u64) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("failed to build multimodal http client");
        Self {
            spec: ToolSpec {
                name: "multimodal_looker".to_string(),
                description: "Analyze media (image/audio/video/document) from a local path or URL. Required: source. Optional: question, media_type (image/audio/video/document/auto), mime_type, detail (low/high/auto for images)."
                    .to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["source"],
                    "properties": {
                        "source": { "type": "string" },
                        "question": { "type": "string" },
                        "media_type": { "type": "string", "enum": ["image", "audio", "video", "document", "auto"] },
                        "mime_type": { "type": "string" },
                        "detail": { "type": "string", "enum": ["low", "high", "auto"] }
                    },
                    "additionalProperties": false
                }),
            },
            agent,
            client,
            max_media_size_bytes,
            max_image_size_bytes,
        }
    }
}

#[async_trait]
impl ToolExecutor for MultimodalLookerTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn required_permissions(
        &self,
        ctx: &ToolContext,
        input: &Value,
    ) -> Result<Vec<Permission>, ToolError> {
        let source = input
            .get("source")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("missing source".to_string()))?;
        if is_url(source) {
            let host = parse_host(source)?;
            Ok(vec![Permission::NetAccess {
                domain: DomainPattern(host),
            }])
        } else {
            let resolved = resolve_path(&ctx.working_dir, ctx.jail_root.as_deref(), source)?;
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
    }

    async fn execute(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        let source = input
            .get("source")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("missing source".to_string()))?;
        let question = input
            .get("question")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("Analyze the provided media.");
        let media_hint = input
            .get("media_type")
            .and_then(Value::as_str)
            .unwrap_or("auto");
        let detail = input
            .get("detail")
            .and_then(Value::as_str)
            .map(parse_detail)
            .transpose()?;
        let mime_override = input.get("mime_type").and_then(Value::as_str);

        let (bytes, mime_type, label, size) = if is_url(source) {
            let host = parse_host(source)?;
            ensure_allowed_url(source, &host, Some(ctx)).await?;
            download_url(&self.client, source, self.max_media_size_bytes).await?
        } else {
            let resolved = resolve_path(&ctx.working_dir, ctx.jail_root.as_deref(), source)?;
            let size = std::fs::metadata(&resolved)
                .map_err(|err| ToolError::new(err.to_string()))?
                .len();
            if size > self.max_media_size_bytes {
                return Err(ToolError::new(format!(
                    "media is too large: {size} bytes (limit {})",
                    self.max_media_size_bytes
                )));
            }
            let bytes = std::fs::read(&resolved).map_err(|err| ToolError::new(err.to_string()))?;
            if bytes.len() as u64 > self.max_media_size_bytes {
                return Err(ToolError::new(format!(
                    "media is too large: {} bytes (limit {})",
                    bytes.len(),
                    self.max_media_size_bytes
                )));
            }
            let mime_type = mime_override
                .map(|value| value.to_string())
                .or_else(|| infer_mime_type(&resolved))
                .ok_or_else(|| ToolError::new("missing or unsupported mime_type".to_string()))?;
            (bytes, mime_type, resolved.display().to_string(), size)
        };

        let mime_type = mime_override
            .map(|value| value.to_string())
            .unwrap_or(mime_type);
        let media_kind = resolve_media_kind(media_hint, &mime_type)?;
        let detail = detail.unwrap_or(ImageDetail::Auto);

        if matches!(media_kind, MediaKind::Image) && size > self.max_image_size_bytes {
            return Err(ToolError::new(format!(
                "image is too large: {} bytes (limit {})",
                size, self.max_image_size_bytes
            )));
        }

        let encoded = BASE64_STANDARD.encode(bytes);
        let user_content = build_user_content(media_kind, &mime_type, encoded, detail)?;
        let content = OneOrMany::many(vec![UserContent::text(question), user_content])
            .map_err(|_| ToolError::new("failed to build multimodal prompt".to_string()))?;
        let message = Message::User { content };

        let response = self
            .agent
            .prompt_message_with_retry(message, DEFAULT_PROVIDER_RETRIES)
            .await
            .map_err(|err| ToolError::new(err.to_string()))?;

        Ok(json!({
            "description": response,
            "mime_type": mime_type,
            "bytes": size,
            "media_type": media_kind_label(media_kind),
            "source": label
        }))
    }
}

fn is_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

fn parse_detail(value: &str) -> Result<ImageDetail, ToolError> {
    match value {
        "low" => Ok(ImageDetail::Low),
        "high" => Ok(ImageDetail::High),
        "auto" => Ok(ImageDetail::Auto),
        _ => Err(ToolError::new("invalid detail".to_string())),
    }
}

fn media_kind_label(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => "image",
        MediaKind::Audio => "audio",
        MediaKind::Video => "video",
        MediaKind::Document => "document",
    }
}

fn resolve_media_kind(hint: &str, mime_type: &str) -> Result<MediaKind, ToolError> {
    match hint {
        "image" => Ok(MediaKind::Image),
        "audio" => Ok(MediaKind::Audio),
        "video" => Ok(MediaKind::Video),
        "document" => Ok(MediaKind::Document),
        "auto" => infer_kind_from_mime(mime_type),
        _ => Err(ToolError::new("invalid media_type".to_string())),
    }
}

fn infer_kind_from_mime(mime_type: &str) -> Result<MediaKind, ToolError> {
    if ImageMediaType::from_mime_type(mime_type).is_some() {
        return Ok(MediaKind::Image);
    }
    if AudioMediaType::from_mime_type(mime_type).is_some() {
        return Ok(MediaKind::Audio);
    }
    if VideoMediaType::from_mime_type(mime_type).is_some() {
        return Ok(MediaKind::Video);
    }
    if DocumentMediaType::from_mime_type(mime_type).is_some() {
        return Ok(MediaKind::Document);
    }
    Err(ToolError::new("unsupported mime_type".to_string()))
}

fn build_user_content(
    kind: MediaKind,
    mime_type: &str,
    encoded: String,
    detail: ImageDetail,
) -> Result<UserContent, ToolError> {
    match kind {
        MediaKind::Image => {
            let media_type = ImageMediaType::from_mime_type(mime_type)
                .ok_or_else(|| ToolError::new("unsupported image mime_type".to_string()))?;
            Ok(UserContent::image_base64(
                encoded,
                Some(media_type),
                Some(detail),
            ))
        }
        MediaKind::Audio => {
            let media_type = AudioMediaType::from_mime_type(mime_type)
                .ok_or_else(|| ToolError::new("unsupported audio mime_type".to_string()))?;
            Ok(UserContent::audio(encoded, Some(media_type)))
        }
        MediaKind::Video => {
            let media_type = VideoMediaType::from_mime_type(mime_type)
                .ok_or_else(|| ToolError::new("unsupported video mime_type".to_string()))?;
            Ok(UserContent::Video(rig::completion::message::Video {
                data: rig::completion::message::DocumentSourceKind::Base64(encoded),
                media_type: Some(media_type),
                additional_params: None,
            }))
        }
        MediaKind::Document => {
            let media_type = DocumentMediaType::from_mime_type(mime_type)
                .ok_or_else(|| ToolError::new("unsupported document mime_type".to_string()))?;
            Ok(UserContent::Document(rig::completion::message::Document {
                data: rig::completion::message::DocumentSourceKind::Base64(encoded),
                media_type: Some(media_type),
                additional_params: None,
            }))
        }
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
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "rtf" => "text/rtf",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "xml" => "text/xml",
        "js" => "application/x-javascript",
        "py" => "application/x-python",
        "wav" => "audio/wav",
        "mp3" => "audio/mp3",
        "aiff" | "aif" => "audio/aiff",
        "aac" => "audio/aac",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "avi" => "video/avi",
        "mp4" => "video/mp4",
        "mpeg" | "mpg" => "video/mpeg",
        _ => return None,
    };
    Some(mime.to_string())
}

async fn download_url(
    client: &Client,
    url: &str,
    max_size_bytes: u64,
) -> Result<(Vec<u8>, String, String, u64), ToolError> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| ToolError::new(err.to_string()))?;
    if response.status().is_redirection() {
        return Err(ToolError::new("redirects are not allowed".to_string()));
    }
    let mut size_hint = response.content_length();
    if let Some(length) = size_hint
        && length > max_size_bytes
    {
        return Err(ToolError::new(format!(
            "media is too large: {length} bytes (limit {max_size_bytes})"
        )));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or(value).trim().to_string());
    let bytes = response
        .bytes()
        .await
        .map_err(|err| ToolError::new(err.to_string()))?;
    if bytes.len() as u64 > max_size_bytes {
        return Err(ToolError::new(format!(
            "media is too large: {} bytes (limit {max_size_bytes})",
            bytes.len()
        )));
    }
    if size_hint.is_none() {
        size_hint = Some(bytes.len() as u64);
    }
    let mime = content_type.ok_or_else(|| ToolError::new("missing content-type".to_string()))?;
    Ok((
        bytes.to_vec(),
        mime,
        url.to_string(),
        size_hint.unwrap_or(bytes.len() as u64),
    ))
}
