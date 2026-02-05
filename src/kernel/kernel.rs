use std::sync::Arc;

use serde_json::Value;

use crate::kernel::permissions::{CapabilitySet, Permission};

#[derive(Debug, Default, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub capabilities: Arc<CapabilitySet>,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ToolError {
    message: String,
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ToolError {}

pub type ToolOutput = Value;

pub trait ToolExecutor: Send + Sync {
    fn spec(&self) -> &ToolSpec;
    fn required_permissions(
        &self,
        ctx: &ToolContext,
        input: &Value,
    ) -> Result<Vec<Permission>, ToolError>;
    fn execute(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError>;
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<Arc<dyn ToolExecutor>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    pub fn register(&mut self, tool: Arc<dyn ToolExecutor>) {
        self.tools.push(tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn ToolExecutor>> {
        self.tools
            .iter()
            .find(|tool| tool.spec().name == name)
            .cloned()
    }

    pub fn validate_input(
        &self,
        _tool: &dyn ToolExecutor,
        _input: &Value,
    ) -> Result<(), ToolError> {
        Ok(())
    }

    pub fn required_permissions(
        &self,
        tool: &dyn ToolExecutor,
        ctx: &ToolContext,
        input: &Value,
    ) -> Result<Vec<Permission>, ToolError> {
        tool.required_permissions(ctx, input)
    }
}

pub struct Kernel {
    tool_registry: Arc<ToolRegistry>,
    context: ToolContext,
}

impl Kernel {
    pub fn new(tool_registry: ToolRegistry) -> Self {
        Self {
            tool_registry: Arc::new(tool_registry),
            context: ToolContext {
                capabilities: Arc::new(CapabilitySet::empty()),
                user_id: None,
                session_id: None,
            },
        }
    }

    pub fn with_capabilities(mut self, capabilities: CapabilitySet) -> Self {
        self.context.capabilities = Arc::new(capabilities);
        self
    }

    pub fn clone_with_context(&self, user_id: Option<String>, session_id: Option<String>) -> Self {
        let mut context = self.context.clone();
        context.user_id = user_id;
        context.session_id = session_id;
        Self {
            tool_registry: Arc::clone(&self.tool_registry),
            context,
        }
    }

    pub fn tool_registry(&self) -> &ToolRegistry {
        self.tool_registry.as_ref()
    }

    pub fn context(&self) -> &ToolContext {
        &self.context
    }

    pub fn invoke_tool(
        &self,
        tool: &dyn ToolExecutor,
        input: Value,
    ) -> Result<ToolOutput, ToolError> {
        self.invoke_tool_with_grants(tool, input, None)
    }

    pub fn invoke_tool_with_grants(
        &self,
        tool: &dyn ToolExecutor,
        input: Value,
        extra_grants: Option<&CapabilitySet>,
    ) -> Result<ToolOutput, ToolError> {
        self.tool_registry.validate_input(tool, &input)?;

        let required = self
            .tool_registry
            .required_permissions(tool, &self.context, &input)?;
        let allowed = self.context.capabilities.allows_all(&required)
            || extra_grants
                .map(|grants| grants.allows_all(&required))
                .unwrap_or(false)
            || required
                .iter()
                .all(|permission| permission.is_auto_granted(&self.context));
        if !allowed {
            return Err(ToolError {
                message: format!("permission denied for tool '{}'", tool.spec().name),
            });
        }
        if let Some(grants) = extra_grants {
            let mut merged = self.context.capabilities.as_ref().clone();
            for permission in grants.permissions() {
                merged.insert(permission.clone());
            }
            let mut scoped = self.context.clone();
            scoped.capabilities = Arc::new(merged);
            tool.execute(&scoped, input)
        } else {
            tool.execute(&self.context, input)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::{Kernel, ToolContext, ToolError, ToolExecutor, ToolOutput, ToolRegistry, ToolSpec};
    use crate::kernel::permissions::{CapabilitySet, PathPattern, Permission};

    #[derive(Debug)]
    struct DummyTool {
        spec: ToolSpec,
    }

    impl DummyTool {
        fn new() -> Self {
            Self {
                spec: ToolSpec {
                    name: "dummy".to_string(),
                    description: "dummy tool".to_string(),
                    schema: json!({"type": "object"}),
                },
            }
        }
    }

    impl ToolExecutor for DummyTool {
        fn spec(&self) -> &ToolSpec {
            &self.spec
        }

        fn required_permissions(
            &self,
            _ctx: &ToolContext,
            _input: &serde_json::Value,
        ) -> Result<Vec<Permission>, ToolError> {
            Ok(vec![Permission::FileRead {
                path: PathPattern("/tmp/allowed.txt".to_string()),
            }])
        }

        fn execute(
            &self,
            _ctx: &ToolContext,
            _input: serde_json::Value,
        ) -> Result<ToolOutput, ToolError> {
            Ok(json!({"status": "ok"}))
        }
    }

    #[test]
    fn invoke_tool_denies_without_permission() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(DummyTool::new()));

        let kernel = Kernel::new(registry);
        let tool = kernel.tool_registry().get("dummy").unwrap();
        let result = kernel.invoke_tool(tool.as_ref(), json!({}));

        assert!(result.is_err());
    }

    #[test]
    fn invoke_tool_allows_with_permission() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(DummyTool::new()));

        let mut capabilities = CapabilitySet::empty();
        capabilities.insert(Permission::FileRead {
            path: PathPattern("/tmp/allowed.txt".to_string()),
        });

        let kernel = Kernel::new(registry).with_capabilities(capabilities);
        let tool = kernel.tool_registry().get("dummy").unwrap();
        let result = kernel.invoke_tool(tool.as_ref(), json!({}));

        assert!(result.is_ok());
    }
}
