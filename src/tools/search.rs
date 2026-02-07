use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{Value, json};

use crate::config::SearchConfig;
use crate::kernel::permissions::{DomainPattern, Permission};
use crate::tools::net_utils::{ensure_allowed_url, parse_host, read_response_bytes};
use crate::tools::traits::{ToolContext, ToolError, ToolExecutor, ToolOutput, ToolSpec};

const DEFAULT_MAX_RESULTS: usize = 5;
const DEFAULT_MAX_SNIPPET_CHARS: usize = 2000;
const DEFAULT_MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
const ERROR_BODY_BYTES: u64 = 16 * 1024;
const GOOGLE_SEARCH_BASE_URL: &str = "https://www.googleapis.com/customsearch/v1";
const SEARXNG_SEARCH_PATH: &str = "/search";

#[derive(Debug)]
pub struct SearchTool {
    spec: ToolSpec,
    client: Client,
    provider: SearchProvider,
    max_results: usize,
    max_snippet_chars: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchProviderKind {
    Google,
    Searxng,
}

#[derive(Debug)]
struct SearchProvider {
    kind: SearchProviderKind,
    api_key: Option<String>,
    engine_id: Option<String>,
    base_urls: Vec<String>,
    allow_private_base_urls: bool,
    searxng_engines: Option<String>,
    searxng_categories: Option<String>,
    searxng_safesearch: Option<u8>,
}

impl SearchTool {
    pub fn new(config: &SearchConfig) -> Result<Self, ToolError> {
        let provider = config.provider.as_deref().unwrap_or("google");
        let kind = match provider.trim().to_ascii_lowercase().as_str() {
            "google" => SearchProviderKind::Google,
            "searxng" => SearchProviderKind::Searxng,
            _ => return Err(ToolError::new("unsupported search provider".to_string())),
        };

        let api_key = match kind {
            SearchProviderKind::Google => {
                let api_key_env = config
                    .api_key_env
                    .as_deref()
                    .unwrap_or("GOOGLE_CSE_API_KEY");
                Some(
                    std::env::var(api_key_env)
                        .map_err(|_| ToolError::new(format!("missing API key in env '{api_key_env}'")))?,
                )
            }
            SearchProviderKind::Searxng => None,
        };
        let engine_id = match kind {
            SearchProviderKind::Google => Some(
                config
                    .engine_id
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| ToolError::new("search.engine_id is required".to_string()))?
                    .to_string(),
            ),
            SearchProviderKind::Searxng => None,
        };
        let mut base_urls = Vec::new();
        if let Some(base_url) = config.base_url.as_deref() {
            if !base_url.trim().is_empty() {
                base_urls.push(base_url.to_string());
            }
        }
        if let Some(list) = &config.base_urls {
            for entry in list {
                if !entry.trim().is_empty() {
                    base_urls.push(entry.trim().to_string());
                }
            }
        }
        if base_urls.is_empty() {
            base_urls.push(GOOGLE_SEARCH_BASE_URL.to_string());
        }
        let max_results = config
            .max_results
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .clamp(1, 10);
        let max_snippet_chars = config
            .max_snippet_chars
            .unwrap_or(DEFAULT_MAX_SNIPPET_CHARS);

        let client = Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|err| ToolError::new(err.to_string()))?;

        Ok(Self {
            spec: ToolSpec {
                name: "web_search".to_string(),
                description: "Search the web via a configured search provider (Google Custom Search). Returns a short list of results with title, url, and snippet. Use http_fetch to retrieve full pages when needed."
                    .to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["query"],
                    "properties": {
                        "query": { "type": "string", "minLength": 1, "maxLength": 400 },
                        "count": { "type": "integer", "minimum": 1, "maximum": 10, "default": 5 },
                        "freshness": { "type": "string", "enum": ["day", "week", "month", "year"] }
                    },
                    "additionalProperties": false
                }),
            },
            client,
            provider: SearchProvider {
                kind,
                api_key,
                engine_id,
                base_urls: base_urls.iter().map(|value| value.trim_end_matches('/').to_string()).collect(),
                allow_private_base_urls: config.allow_private_base_urls.unwrap_or(false),
                searxng_engines: config.searxng_engines.clone(),
                searxng_categories: config.searxng_categories.clone(),
                searxng_safesearch: config.searxng_safesearch,
            },
            max_results,
            max_snippet_chars,
        })
    }
}

#[async_trait]
impl ToolExecutor for SearchTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn required_permissions(
        &self,
        _ctx: &ToolContext,
        _input: &Value,
    ) -> Result<Vec<Permission>, ToolError> {
        let mut permissions = Vec::new();
        for base_url in &self.provider.base_urls {
            let host = parse_host(base_url)?;
            permissions.push(Permission::NetAccess {
                domain: DomainPattern(host),
            });
        }
        Ok(permissions)
    }

    async fn execute(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        let query = input
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("missing query".to_string()))?;
        let count = input
            .get("count")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(self.max_results)
            .clamp(1, 10);
        let freshness = input.get("freshness").and_then(Value::as_str);

        let (base_url, payload) = self.fetch_with_fallbacks(ctx, query, count, freshness).await?;
        let mut results = Vec::new();
        match self.provider.kind {
            SearchProviderKind::Google => {
                let items = payload.get("items").and_then(Value::as_array);
                if let Some(items) = items {
                    for (idx, item) in items.iter().take(count).enumerate() {
                        let title = item.get("title").and_then(Value::as_str).unwrap_or("");
                        let url = item.get("link").and_then(Value::as_str).unwrap_or("");
                        let raw_snippet = item
                            .get("snippet")
                            .and_then(Value::as_str)
                            .or_else(|| item.get("htmlSnippet").and_then(Value::as_str));
                        let snippet = raw_snippet
                            .map(strip_google_snippet)
                            .map(|value| truncate_chars(&value, self.max_snippet_chars))
                            .unwrap_or_default();
                        let display_url = item.get("displayLink").and_then(Value::as_str);
                        results.push(json!({
                            "index": idx + 1,
                            "title": title,
                            "url": url,
                            "display_url": display_url,
                            "snippet": snippet
                        }));
                    }
                }
            }
            SearchProviderKind::Searxng => {
                let items = payload.get("results").and_then(Value::as_array);
                if let Some(items) = items {
                    for (idx, item) in items.iter().take(count).enumerate() {
                        let title = item.get("title").and_then(Value::as_str).unwrap_or("");
                        let url = item.get("url").and_then(Value::as_str).unwrap_or("");
                        let raw_snippet = item
                            .get("content")
                            .and_then(Value::as_str)
                            .or_else(|| item.get("snippet").and_then(Value::as_str));
                        let snippet = raw_snippet
                            .map(strip_google_snippet)
                            .map(|value| truncate_chars(&value, self.max_snippet_chars))
                            .unwrap_or_default();
                        let display_url = item.get("pretty_url").and_then(Value::as_str);
                        results.push(json!({
                            "index": idx + 1,
                            "title": title,
                            "url": url,
                            "display_url": display_url,
                            "snippet": snippet
                        }));
                    }
                }
            }
        }

        Ok(json!({
            "query": query,
            "provider": match self.provider.kind {
                SearchProviderKind::Google => "google",
                SearchProviderKind::Searxng => "searxng",
            },
            "base_url": base_url,
            "result_count": results.len(),
            "results": results
        }))
    }
}

impl SearchTool {
    async fn fetch_with_fallbacks(
        &self,
        ctx: &ToolContext,
        query: &str,
        count: usize,
        freshness: Option<&str>,
    ) -> Result<(String, Value), ToolError> {
        let mut last_error = None;
        for base_url in &self.provider.base_urls {
            match self
                .fetch_from_base_url(ctx, base_url, query, count, freshness)
                .await
            {
                Ok(payload) => return Ok((base_url.clone(), payload)),
                Err(err) => last_error = Some(err),
            }
        }
        Err(last_error.unwrap_or_else(|| ToolError::new("search failed".to_string())))
    }

    async fn fetch_from_base_url(
        &self,
        ctx: &ToolContext,
        base_url: &str,
        query: &str,
        count: usize,
        freshness: Option<&str>,
    ) -> Result<Value, ToolError> {
        let host = parse_host(base_url)?;
        if !self.provider.allow_private_base_urls {
            ensure_allowed_url(base_url, &host, Some(ctx)).await?;
        }

        let request = match self.provider.kind {
            SearchProviderKind::Google => self.build_google_request(base_url, query, count, freshness)?,
            SearchProviderKind::Searxng => self.build_searxng_request(base_url, query, count, freshness)?,
        };

        let response = request
            .send()
            .await
            .map_err(|err| ToolError::new(err.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body_bytes = read_response_bytes(response, ERROR_BODY_BYTES, "search error")
                .await
                .unwrap_or_default();
            let body = String::from_utf8_lossy(&body_bytes).to_string();
            return Err(ToolError::new(format!(
                "search request failed with status {status}: {body}"
            )));
        }

        let max_bytes = ctx.max_response_bytes.unwrap_or(DEFAULT_MAX_RESPONSE_BYTES);
        let body_bytes = read_response_bytes(response, max_bytes, "search response")
            .await
            .map_err(|err| ToolError::new(err.to_string()))?;
        serde_json::from_slice(&body_bytes).map_err(|err| ToolError::new(err.to_string()))
    }

    fn build_google_request(
        &self,
        base_url: &str,
        query: &str,
        count: usize,
        freshness: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, ToolError> {
        let api_key = self
            .provider
            .api_key
            .as_ref()
            .ok_or_else(|| ToolError::new("missing search API key".to_string()))?;
        let engine_id = self
            .provider
            .engine_id
            .as_ref()
            .ok_or_else(|| ToolError::new("search.engine_id is required".to_string()))?;
        let mut params = vec![
            ("key".to_string(), api_key.clone()),
            ("cx".to_string(), engine_id.clone()),
            ("q".to_string(), query.to_string()),
            ("num".to_string(), count.to_string()),
        ];
        if let Some(freshness) = freshness {
            let sort_value = match freshness {
                "day" => "date:r:1d",
                "week" => "date:r:7d",
                "month" => "date:r:30d",
                "year" => "date:r:365d",
                _ => "",
            };
            if !sort_value.is_empty() {
                params.push(("sort".to_string(), sort_value.to_string()));
            }
        }
        let request = self.client.get(base_url).query(&params);
        Ok(request)
    }

    fn build_searxng_request(
        &self,
        base_url: &str,
        query: &str,
        count: usize,
        freshness: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, ToolError> {
        let mut url = base_url.to_string();
        url.push_str(SEARXNG_SEARCH_PATH);
        let mut params = vec![
            ("q".to_string(), query.to_string()),
            ("format".to_string(), "json".to_string()),
            ("pageno".to_string(), "1".to_string()),
            ("count".to_string(), count.to_string()),
        ];
        if let Some(engines) = self.provider.searxng_engines.as_deref() {
            params.push(("engines".to_string(), engines.to_string()));
        }
        if let Some(categories) = self.provider.searxng_categories.as_deref() {
            params.push(("categories".to_string(), categories.to_string()));
        }
        if let Some(safesearch) = self.provider.searxng_safesearch {
            params.push(("safesearch".to_string(), safesearch.to_string()));
        }
        if let Some(freshness) = freshness {
            let time_range = match freshness {
                "day" => "day",
                "week" => "week",
                "month" => "month",
                "year" => "year",
                _ => "",
            };
            if !time_range.is_empty() {
                params.push(("time_range".to_string(), time_range.to_string()));
            }
        }
        Ok(self.client.get(&url).query(&params))
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
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
        return value.to_string();
    }
    let mut output = value[..end].to_string();
    output.push_str("\n\n[truncated]");
    output
}

fn strip_google_snippet(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut in_tag = false;
    for ch in value.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }
    output
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{SearchTool, truncate_chars};
    use crate::config::SearchConfig;
    use crate::kernel::permissions::{CapabilitySet, Permission};
    use crate::tools::traits::{ExecutionMode, ToolContext, ToolExecutor};

    #[test]
    fn truncate_chars_respects_limit() {
        let value = truncate_chars("abcdef", 3);
        assert!(value.starts_with("abc"));
        assert!(value.contains("[truncated]"));
    }

    #[test]
    fn required_permissions_use_base_url_host() {
        let config = SearchConfig {
            provider: Some("searxng".to_string()),
            api_key_env: None,
            engine_id: None,
            base_url: Some("https://searx.example.com".to_string()),
            base_urls: None,
            allow_private_base_urls: None,
            max_results: None,
            max_snippet_chars: None,
            searxng_engines: None,
            searxng_categories: None,
            searxng_safesearch: None,
        };
        let tool = SearchTool::new(&config).unwrap();
        let ctx = ToolContext {
            capabilities: std::sync::Arc::new(CapabilitySet::empty()),
            user_id: None,
            session_id: None,
            channel_id: None,
            working_dir: std::path::PathBuf::from("/"),
            jail_root: None,
            scheduler: None,
            notifications: None,
            notify_tool_used: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            execution_mode: ExecutionMode::User,
            timezone_offset: "+00:00".to_string(),
            timezone_name: "UTC".to_string(),
            max_response_bytes: None,
            max_response_chars: None,
        };
        let required = tool.required_permissions(&ctx, &json!({"query": "test"})).unwrap();
        assert!(matches!(
            required[0],
            Permission::NetAccess { .. }
        ));
    }

    #[test]
    fn strip_google_snippet_removes_tags() {
        let value = super::strip_google_snippet("hello <b>world</b> &amp; &lt;ok&gt;");
        assert_eq!(value, "hello world & <ok>");
    }

    #[test]
    fn allow_private_base_urls_skips_ssrf_check() {
        let config = SearchConfig {
            provider: Some("searxng".to_string()),
            api_key_env: None,
            engine_id: None,
            base_url: Some("http://127.0.0.1:8888".to_string()),
            base_urls: None,
            allow_private_base_urls: Some(true),
            max_results: None,
            max_snippet_chars: None,
            searxng_engines: None,
            searxng_categories: None,
            searxng_safesearch: None,
        };
        let tool = SearchTool::new(&config).unwrap();
        assert!(tool.provider.allow_private_base_urls);
    }
}
