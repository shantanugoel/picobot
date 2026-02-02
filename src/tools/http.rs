use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{Value, json};

use crate::kernel::permissions::Permission;
use crate::tools::traits::{Tool, ToolContext, ToolError, ToolOutput};

#[derive(Debug)]
pub struct HttpTool {
    client: Client,
}

impl HttpTool {
    pub fn new() -> Result<Self, ToolError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl Tool for HttpTool {
    fn name(&self) -> &'static str {
        "http_fetch"
    }

    fn description(&self) -> &'static str {
        "Fetch a URL with domain allowlist"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["url"],
            "properties": {
                "url": { "type": "string" },
                "method": { "type": "string", "enum": ["GET", "POST"] },
                "headers": { "type": "object" },
                "body": { "type": "string" }
            },
            "additionalProperties": false
        })
    }

    fn required_permissions(
        &self,
        _ctx: &ToolContext,
        input: &Value,
    ) -> Result<Vec<Permission>, ToolError> {
        let url = input
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("missing url".to_string()))?;
        let host = parse_host(url)?;
        let permission = Permission::NetAccess {
            domain: crate::kernel::permissions::DomainPattern(host),
        };
        Ok(vec![permission])
    }

    async fn execute(&self, _ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        let url = input
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("missing url".to_string()))?;
        let method = input.get("method").and_then(Value::as_str).unwrap_or("GET");
        let headers = input.get("headers").and_then(Value::as_object);
        let body = input.get("body").and_then(Value::as_str);

        let _host = parse_host(url)?;

        let mut request = match method {
            "GET" => self.client.get(url),
            "POST" => self.client.post(url),
            _ => return Err(ToolError::InvalidInput("invalid method".to_string())),
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
            .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;

        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;

        Ok(json!({
            "status": status,
            "body": text
        }))
    }
}

fn parse_host(url: &str) -> Result<String, ToolError> {
    let parsed =
        reqwest::Url::parse(url).map_err(|err| ToolError::InvalidInput(err.to_string()))?;
    parsed
        .host_str()
        .map(|host| host.to_string())
        .ok_or_else(|| ToolError::InvalidInput("missing host".to_string()))
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
