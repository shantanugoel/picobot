use std::collections::HashMap;

use serde_json::Value;

use crate::tools::schema;
use crate::tools::traits::{Tool, ToolError, ToolSpec};

#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) -> Result<(), ToolRegistryError> {
        let name = tool.name().to_string();
        if self.tools.contains_key(&name) {
            return Err(ToolRegistryError::DuplicateTool(name));
        }
        self.tools.insert(name, tool);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|tool| tool.as_ref())
    }

    pub fn validate_input(&self, tool: &dyn Tool, input: &Value) -> Result<(), ToolError> {
        schema::validate(&tool.schema(), input)
    }

    pub fn required_permissions(
        &self,
        tool: &dyn Tool,
        ctx: &crate::kernel::context::ToolContext,
        input: &Value,
    ) -> Result<Vec<crate::kernel::permissions::Permission>, ToolError> {
        tool.required_permissions(ctx, input)
    }

    pub fn tool_specs(&self) -> Vec<ToolSpec> {
        self.tools
            .values()
            .map(|tool| ToolSpec {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                schema: tool.schema(),
            })
            .collect()
    }

    pub fn tools(&self) -> Vec<&dyn Tool> {
        self.tools.values().map(|tool| tool.as_ref()).collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ToolRegistryError {
    #[error("Tool '{0}' already registered")]
    DuplicateTool(String),
    #[error("Failed to initialize builtin tool '{tool}': {detail}")]
    BuiltinToolInitFailed { tool: String, detail: String },
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::json;

    use crate::kernel::context::ToolContext;
    use crate::kernel::permissions::{CapabilitySet, Permission};
    use crate::tools::registry::ToolRegistry;
    use crate::tools::traits::{Tool, ToolError, ToolOutput};

    #[derive(Debug)]
    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &'static str {
            "echo"
        }

        fn description(&self) -> &'static str {
            "echo tool"
        }

        fn schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "required": ["value"],
                "properties": {
                    "value": { "type": "string" }
                },
                "additionalProperties": false
            })
        }

        fn required_permissions(
            &self,
            _ctx: &ToolContext,
            _input: &serde_json::Value,
        ) -> Result<Vec<Permission>, ToolError> {
            Ok(vec![])
        }

        async fn execute(
            &self,
            _ctx: &ToolContext,
            input: serde_json::Value,
        ) -> Result<ToolOutput, ToolError> {
            Ok(input)
        }
    }

    #[tokio::test]
    async fn registry_validates_schema() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool)).unwrap();
        let tool = registry.get("echo").unwrap();

        let good = json!({"value": "ok"});
        let bad = json!({"value": 123});

        assert!(registry.validate_input(tool, &good).is_ok());
        assert!(registry.validate_input(tool, &bad).is_err());
    }

    #[tokio::test]
    async fn registry_returns_required_permissions() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool)).unwrap();
        let tool = registry.get("echo").unwrap();

        let ctx = ToolContext {
            working_dir: std::path::PathBuf::from("/"),
            capabilities: std::sync::Arc::new(CapabilitySet::empty()),
            user_id: None,
            session_id: None,
            scheduler: std::sync::Arc::new(std::sync::RwLock::new(None)),
            log_model_requests: false,
            include_tool_messages: true,
        };

        let perms = registry
            .required_permissions(tool, &ctx, &json!({"value": "ok"}))
            .unwrap();
        assert!(perms.is_empty());
    }
}
