use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{Value, json};

use crate::kernel::permissions::{DomainPattern, Permission};
use crate::tools::net_utils::{ensure_allowed_url, parse_host};
use crate::tools::traits::{ToolContext, ToolError, ToolExecutor, ToolOutput, ToolSpec};

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
                description: "Fetch a URL with domain allowlist. Required: url. Optional: method (GET/POST), headers, body."
                    .to_string(),
                schema: json!({
                    "type": "object",
                    "required": ["url"],
                    "properties": {
                        "url": { "type": "string" },
                        "method": { "type": "string", "enum": ["GET", "POST"] },
                        "headers": { "type": "object" },
                        "body": { "type": "string" }
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

    async fn execute(&self, _ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        let url = input
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("missing url".to_string()))?;
        let method = input.get("method").and_then(Value::as_str).unwrap_or("GET");
        let headers = input.get("headers").and_then(Value::as_object);
        let body = input.get("body").and_then(Value::as_str);

        let host = parse_host(url)?;
        ensure_allowed_url(url, &host).await?;

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
        let text = response
            .text()
            .await
            .map_err(|err| ToolError::new(err.to_string()))?;

        Ok(json!({
            "status": status,
            "body": text
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use crate::tools::net_utils::{is_private_ip, parse_host};

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
}
