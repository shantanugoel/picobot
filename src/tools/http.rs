use std::net::IpAddr;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{Value, json};

use crate::kernel::permissions::{DomainPattern, Permission};
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

fn parse_host(url: &str) -> Result<String, ToolError> {
    let parsed = reqwest::Url::parse(url).map_err(|err| ToolError::new(err.to_string()))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err(ToolError::new("unsupported URL scheme".to_string())),
    }
    parsed
        .host_str()
        .map(|host| host.to_string())
        .ok_or_else(|| ToolError::new("missing host".to_string()))
}

async fn ensure_allowed_url(url: &str, host: &str) -> Result<(), ToolError> {
    let parsed = reqwest::Url::parse(url).map_err(|err| ToolError::new(err.to_string()))?;
    if parsed.username() != "" || parsed.password().is_some() {
        return Err(ToolError::new(
            "credentials in URL are not allowed".to_string(),
        ));
    }
    let port = parsed.port_or_known_default().unwrap_or(80);
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|err| ToolError::new(err.to_string()))?;
    for addr in addrs {
        if is_private_ip(addr.ip()) {
            return Err(ToolError::new(format!(
                "SSRF blocked: {host} resolves to private IP {}",
                addr.ip()
            )));
        }
    }
    Ok(())
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.octets()[0] == 169
        }
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_host;

    #[test]
    fn parse_host_extracts_domain() {
        let host = parse_host("https://example.com/path").unwrap();
        assert_eq!(host, "example.com");
    }
}
