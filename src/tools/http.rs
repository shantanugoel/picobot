use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::CONTENT_TYPE;
use reqwest::Client;
use serde_json::{Map, Value, json};

use crate::kernel::permissions::{DomainPattern, Permission};
use crate::tools::net_utils::{ensure_allowed_url, parse_host, read_response_bytes};
use crate::tools::traits::{ToolContext, ToolError, ToolExecutor, ToolOutput, ToolSpec};

const DEFAULT_MAX_RESPONSE_BYTES: u64 = 5 * 1024 * 1024;
const DEFAULT_MAX_RESPONSE_CHARS: usize = 50_000;
const HTML_TEXT_WIDTH: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Auto,
    Raw,
    Text,
    MetadataOnly,
}

impl OutputFormat {
    fn parse(value: Option<&str>) -> Result<Self, ToolError> {
        let value = value.unwrap_or("auto").to_ascii_lowercase();
        match value.as_str() {
            "auto" => Ok(Self::Auto),
            "raw" => Ok(Self::Raw),
            "text" => Ok(Self::Text),
            "metadata_only" => Ok(Self::MetadataOnly),
            _ => Err(ToolError::new("invalid output_format".to_string())),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Raw => "raw",
            Self::Text => "text",
            Self::MetadataOnly => "metadata_only",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyMode {
    Raw,
    Text,
    MetadataOnly,
}

impl BodyMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Text => "text",
            Self::MetadataOnly => "metadata_only",
        }
    }
}

#[derive(Debug)]
pub struct HttpTool {
    spec: ToolSpec,
    client: Client,
}

impl HttpTool {
    pub fn new() -> Result<Self, ToolError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|err| ToolError::new(err.to_string()))?;
        Ok(Self {
            spec: ToolSpec {
                name: "http_fetch".to_string(),
                description: "Fetch a URL over HTTP. Only allowlisted domains succeed. Redirects are blocked. Returns status/body plus metadata. Default output_format=auto extracts readable text for HTML, passes through JSON/text, and omits binary bodies. Optional: method (GET/POST), headers, body, output_format, max_body_chars."
                    .to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["url"],
                    "properties": {
                        "url": { "type": "string", "minLength": 8 },
                        "method": { "type": "string", "enum": ["GET", "POST"] },
                        "headers": {
                            "type": "object",
                            "additionalProperties": { "type": "string" }
                        },
                        "body": { "type": "string" },
                        "output_format": {
                            "type": "string",
                            "enum": ["auto", "raw", "text", "metadata_only"],
                            "default": "auto"
                        },
                        "max_body_chars": { "type": "integer", "minimum": 0 }
                    },
                    "additionalProperties": false
                }),
            },
            client,
        })
    }
}

#[async_trait]
impl ToolExecutor for HttpTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn required_permissions(
        &self,
        _ctx: &ToolContext,
        input: &Value,
    ) -> Result<Vec<Permission>, ToolError> {
        let url = input
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("missing url".to_string()))?;
        let host = parse_host(url)?;
        Ok(vec![Permission::NetAccess {
            domain: DomainPattern(host),
        }])
    }

    async fn execute(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        let url = input
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("missing url".to_string()))?;
        let method = input.get("method").and_then(Value::as_str).unwrap_or("GET");
        let headers = input.get("headers").and_then(Value::as_object);
        let body = input.get("body").and_then(Value::as_str);
        let output_format =
            OutputFormat::parse(input.get("output_format").and_then(Value::as_str))?;
        let max_body_chars = parse_max_body_chars(
            input.get("max_body_chars"),
            ctx.max_response_chars,
        )?;

        let host = parse_host(url)?;
        ensure_allowed_url(url, &host, Some(ctx)).await?;

        let mut request = match method {
            "GET" => self.client.get(url),
            "POST" => self.client.post(url),
            _ => return Err(ToolError::new("invalid method".to_string())),
        };

        if let Some(headers) = headers {
            for (key, value) in headers {
                if let Some(value) = value.as_str() {
                    request = request.header(key, value);
                }
            }
        }

        if let Some(body) = body {
            request = request.body(body.to_string());
        }

        let response = request
            .send()
            .await
            .map_err(|err| ToolError::new(err.to_string()))?;
        if response.status().is_redirection() {
            return Err(ToolError::new("redirects are not allowed".to_string()));
        }

        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.to_string());
        let normalized_content_type = content_type
            .as_deref()
            .map(normalize_content_type);
        let content_length = response.content_length();
        let (mode, extract_html) =
            plan_body_format(output_format, normalized_content_type.as_deref())?;

        if mode == BodyMode::MetadataOnly {
            let note = if output_format == OutputFormat::MetadataOnly {
                "metadata_only requested; body omitted".to_string()
            } else {
                "binary content; body omitted (use multimodal_looker for media)".to_string()
            };
            return Ok(build_output(
                status,
                content_type,
                content_length,
                output_format,
                mode,
                None,
                false,
                None,
                Some(note),
            ));
        }

        let max_bytes = ctx.max_response_bytes.unwrap_or(DEFAULT_MAX_RESPONSE_BYTES);
        let bytes = read_response_bytes(response, max_bytes, "response").await?;

        let (body, truncated, title) = match mode {
            BodyMode::Text if extract_html => {
                let text = extract_html_text(&bytes);
                let title = extract_html_title(&bytes);
                let (body, truncated) = truncate_text(&text, max_body_chars);
                (Some(body), truncated, title)
            }
            BodyMode::Text | BodyMode::Raw => {
                let text = String::from_utf8_lossy(&bytes).to_string();
                let (body, truncated) = truncate_text(&text, max_body_chars);
                (Some(body), truncated, None)
            }
            BodyMode::MetadataOnly => (None, false, None),
        };

        Ok(build_output(
            status,
            content_type,
            content_length,
            output_format,
            mode,
            body,
            truncated,
            title,
            None,
        ))
    }
}

fn parse_max_body_chars(
    value: Option<&Value>,
    default: Option<usize>,
) -> Result<usize, ToolError> {
    if let Some(value) = value {
        let raw = value
            .as_u64()
            .ok_or_else(|| ToolError::new("max_body_chars must be a non-negative integer".to_string()))?;
        return usize::try_from(raw)
            .map_err(|_| ToolError::new("max_body_chars is too large".to_string()));
    }
    Ok(default.unwrap_or(DEFAULT_MAX_RESPONSE_CHARS))
}

fn normalize_content_type(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or(value)
        .trim()
        .to_ascii_lowercase()
}

fn is_html_content_type(content_type: &str) -> bool {
    matches!(content_type, "text/html" | "application/xhtml+xml")
}

fn is_text_content_type(content_type: &str) -> bool {
    if content_type.starts_with("text/") {
        return true;
    }
    if matches!(
        content_type,
        "application/json"
            | "application/xml"
            | "application/xhtml+xml"
            | "application/javascript"
            | "application/x-www-form-urlencoded"
            | "text/xml"
            | "text/javascript"
    ) {
        return true;
    }
    content_type.ends_with("+json") || content_type.ends_with("+xml")
}

fn is_binary_content_type(content_type: &str) -> bool {
    if is_text_content_type(content_type) {
        return false;
    }
    content_type.starts_with("image/")
        || content_type.starts_with("audio/")
        || content_type.starts_with("video/")
        || content_type.starts_with("application/")
}

fn plan_body_format(
    requested: OutputFormat,
    content_type: Option<&str>,
) -> Result<(BodyMode, bool), ToolError> {
    if requested == OutputFormat::MetadataOnly {
        return Ok((BodyMode::MetadataOnly, false));
    }

    if let Some(content_type) = content_type
        && is_binary_content_type(content_type)
    {
        if requested == OutputFormat::Text {
            return Err(ToolError::new(
                "binary content is not supported for text output".to_string(),
            ));
        }
        if requested == OutputFormat::Auto {
            return Ok((BodyMode::MetadataOnly, false));
        }
    }

    if requested == OutputFormat::Raw {
        return Ok((BodyMode::Raw, false));
    }

    if let Some(content_type) = content_type
        && is_html_content_type(content_type)
    {
        return Ok((BodyMode::Text, true));
    }

    if let Some(content_type) = content_type
        && is_text_content_type(content_type)
    {
        return Ok((BodyMode::Text, false));
    }

    match requested {
        OutputFormat::Text => Ok((BodyMode::Text, false)),
        _ => Ok((BodyMode::Raw, false)),
    }
}

fn extract_html_text(bytes: &[u8]) -> String {
    let text = html2text::from_read(bytes, HTML_TEXT_WIDTH)
        .unwrap_or_else(|_| String::from_utf8_lossy(bytes).to_string());
    normalize_text(&text)
}

fn extract_html_title(bytes: &[u8]) -> Option<String> {
    let html = String::from_utf8_lossy(bytes);
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let start_tag = lower[start..].find('>')? + start + 1;
    let end = lower[start_tag..].find("</title>")? + start_tag;
    let title = html[start_tag..end].trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

fn normalize_text(value: &str) -> String {
    let mut output = String::new();
    let mut last_blank = false;
    for line in value.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !last_blank && !output.is_empty() {
                output.push('\n');
                output.push('\n');
                last_blank = true;
            }
            continue;
        }
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(trimmed);
        output.push('\n');
        last_blank = false;
    }
    output.trim().to_string()
}


fn truncate_text(value: &str, max_chars: usize) -> (String, bool) {
    if max_chars == 0 {
        return (String::new(), true);
    }
    let mut count = 0usize;
    let mut end = value.len();
    let mut truncated = false;
    for (idx, _) in value.char_indices() {
        if count == max_chars {
            end = idx;
            truncated = true;
            break;
        }
        count += 1;
    }
    if !truncated {
        return (value.to_string(), false);
    }
    let mut output = value[..end].to_string();
    output.push_str("\n\n[truncated]");
    (output, true)
}

fn build_output(
    status: u16,
    content_type: Option<String>,
    content_length: Option<u64>,
    output_format: OutputFormat,
    mode: BodyMode,
    body: Option<String>,
    truncated: bool,
    title: Option<String>,
    note: Option<String>,
) -> ToolOutput {
    let mut output = Map::new();
    output.insert("status".to_string(), json!(status));
    output.insert("output_format".to_string(), json!(output_format.as_str()));
    output.insert("mode".to_string(), json!(mode.as_str()));
    if let Some(content_type) = content_type {
        output.insert("content_type".to_string(), json!(content_type));
    }
    if let Some(content_length) = content_length {
        output.insert("content_length".to_string(), json!(content_length));
    }
    output.insert(
        "body".to_string(),
        body.map_or(Value::Null, Value::String),
    );
    if truncated {
        output.insert("truncated".to_string(), json!(true));
    }
    if let Some(title) = title {
        output.insert("title".to_string(), json!(title));
    }
    if let Some(note) = note {
        output.insert("note".to_string(), json!(note));
    }
    Value::Object(output)
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use serde_json::Value;

    use crate::tools::net_utils::{is_private_ip, parse_host};

    use super::{
        OutputFormat, extract_html_text, extract_html_title, is_binary_content_type,
        normalize_content_type, plan_body_format, truncate_text,
    };

    #[test]
    fn parse_host_extracts_domain() {
        let host = parse_host("https://example.com/path").unwrap();
        assert_eq!(host, "example.com");
    }

    #[test]
    fn is_private_ip_blocks_private_ranges() {
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
    }

    #[test]
    fn normalize_content_type_strips_parameters() {
        let normalized = normalize_content_type("Text/HTML; charset=utf-8");
        assert_eq!(normalized, "text/html");
    }

    #[test]
    fn binary_content_types_are_detected() {
        assert!(is_binary_content_type("image/png"));
        assert!(is_binary_content_type("application/pdf"));
        assert!(!is_binary_content_type("text/plain"));
        assert!(!is_binary_content_type("application/json"));
    }

    #[test]
    fn plan_body_format_auto_binary_returns_metadata_only() {
        let (mode, extract_html) =
            plan_body_format(OutputFormat::Auto, Some("image/png")).unwrap();
        assert_eq!(mode.as_str(), "metadata_only");
        assert!(!extract_html);
    }

    #[test]
    fn plan_body_format_auto_json_returns_text() {
        let (mode, extract_html) =
            plan_body_format(OutputFormat::Auto, Some("application/json")).unwrap();
        assert_eq!(mode.as_str(), "text");
        assert!(!extract_html);
    }

    #[test]
    fn plan_body_format_text_binary_errors() {
        let result = plan_body_format(OutputFormat::Text, Some("image/png"));
        assert!(result.is_err());
    }

    #[test]
    fn extract_html_title_reads_title() {
        let html = b"<html><head><title>Example</title></head><body>Hello</body></html>";
        let title = extract_html_title(html).unwrap();
        assert_eq!(title, "Example");
    }

    #[test]
    fn extract_html_text_removes_markup() {
        let html = b"<html><body><h1>Hello</h1><p>World</p></body></html>";
        let text = extract_html_text(html);
        assert!(text.contains("Hello"));
        assert!(!text.contains("<h1>"));
    }

    #[test]
    fn truncate_text_respects_limit() {
        let (body, truncated) = truncate_text("abcdef", 3);
        assert!(truncated);
        assert!(body.starts_with("abc"));
        assert!(body.contains("[truncated]"));
    }

    #[test]
    fn truncate_text_allows_zero() {
        let (body, truncated) = truncate_text("abcdef", 0);
        assert!(truncated);
        assert!(body.is_empty());
    }

    fn tool_output_text(body: &str, output_format: OutputFormat, content_type: &str) -> Value {
        let (mode, extract_html) = plan_body_format(output_format, Some(content_type)).unwrap();
        let text = if extract_html {
            extract_html_text(body.as_bytes())
        } else {
            body.to_string()
        };
        let (body, truncated) = truncate_text(&text, super::DEFAULT_MAX_RESPONSE_CHARS);
        super::build_output(
            200,
            Some(content_type.to_string()),
            Some(body.len() as u64),
            output_format,
            mode,
            Some(body),
            truncated,
            None,
            None,
        )
    }

    #[test]
    fn auto_html_extracts_text_output() {
        let html = "<html><body><h1>Heading</h1><p>Body</p></body></html>";
        let output = tool_output_text(html, OutputFormat::Auto, "text/html");
        let body = output.get("body").and_then(Value::as_str).unwrap();
        assert!(body.contains("Heading"));
        assert!(!body.contains("<h1>"));
    }

    #[test]
    fn auto_json_passes_through_text() {
        let payload = r#"{"ok":true,"value":42}"#;
        let output = tool_output_text(payload, OutputFormat::Auto, "application/json");
        let body = output.get("body").and_then(Value::as_str).unwrap();
        assert_eq!(body, payload);
    }
}
