use std::sync::Arc;

use serde_json::Value;

use crate::kernel::permissions::CapabilitySet;
use crate::scheduler::service::SchedulerService;
use crate::tools::registry::ToolRegistry;
use crate::tools::traits::{ToolContext, ToolError, ToolExecutor, ToolOutput};

#[derive(Clone)]
pub struct Kernel {
    tool_registry: Arc<ToolRegistry>,
    context: ToolContext,
}

impl Kernel {
    pub fn new(tool_registry: Arc<ToolRegistry>) -> Self {
        Self {
            tool_registry,
            context: ToolContext {
                capabilities: Arc::new(CapabilitySet::empty()),
                user_id: None,
                session_id: None,
                channel_id: None,
                working_dir: std::env::current_dir()
                    .unwrap_or_else(|_| std::path::PathBuf::from(".")),
                jail_root: None,
                scheduler: None,
                scheduled_job: false,
                timezone_offset: "+00:00".to_string(),
                timezone_name: "UTC".to_string(),
            },
        }
    }

    pub fn with_capabilities(mut self, capabilities: CapabilitySet) -> Self {
        self.context.capabilities = Arc::new(capabilities);
        self
    }

    pub fn with_working_dir(mut self, working_dir: std::path::PathBuf) -> Self {
        self.context.working_dir = working_dir;
        self
    }

    pub fn with_jail_root(mut self, jail_root: Option<std::path::PathBuf>) -> Self {
        self.context.jail_root = jail_root;
        self
    }

    pub fn with_scheduler(mut self, scheduler: Option<Arc<SchedulerService>>) -> Self {
        self.context.scheduler = scheduler;
        self
    }

    pub fn with_channel_id(mut self, channel_id: Option<String>) -> Self {
        self.context.channel_id = channel_id;
        self
    }

    #[allow(dead_code)]
    pub fn with_timezone(mut self, offset: String, name: String) -> Self {
        self.context.timezone_offset = offset;
        self.context.timezone_name = name;
        self
    }

    pub fn with_scheduled_job_mode(mut self, scheduled_job: bool) -> Self {
        self.context.scheduled_job = scheduled_job;
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

    pub async fn invoke_tool(
        &self,
        tool: &dyn ToolExecutor,
        input: Value,
    ) -> Result<ToolOutput, ToolError> {
        self.invoke_tool_with_grants(tool, input, None).await
    }

    pub async fn invoke_tool_by_name(
        &self,
        name: &str,
        input: Value,
    ) -> Result<ToolOutput, ToolError> {
        let tool = self
            .tool_registry
            .get(name)
            .ok_or_else(|| ToolError::new(format!("unknown tool '{name}'")))?;
        self.invoke_tool(tool.as_ref(), input).await
    }

    pub async fn invoke_tool_with_grants(
        &self,
        tool: &dyn ToolExecutor,
        input: Value,
        extra_grants: Option<&CapabilitySet>,
    ) -> Result<ToolOutput, ToolError> {
        self.tool_registry.validate_input(tool, &input)?;

        let required = self
            .tool_registry
            .required_permissions(tool, &self.context, &input)?;
        let allowed = match tool.spec().name.as_str() {
            "schedule" => {
                self.context.capabilities.allows_any(&required)
                    || extra_grants
                        .map(|grants| grants.allows_any(&required))
                        .unwrap_or(false)
            }
            _ => {
                self.context.capabilities.allows_all(&required)
                    || extra_grants
                        .map(|grants| grants.allows_all(&required))
                        .unwrap_or(false)
            }
        } || required
            .iter()
            .all(|permission| permission.is_auto_granted(&self.context));
        if !allowed {
            return Err(ToolError::new(format!(
                "permission denied for tool '{}'",
                tool.spec().name
            )));
        }
        if let Some(grants) = extra_grants {
            let mut merged = self.context.capabilities.as_ref().clone();
            for permission in grants.permissions() {
                merged.insert(permission.clone());
            }
            let mut scoped = self.context.clone();
            scoped.capabilities = Arc::new(merged);
            tool.execute(&scoped, input).await
        } else {
            tool.execute(&self.context, input).await
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::json;

    use super::Kernel;
    use crate::kernel::permissions::{CapabilitySet, PathPattern, Permission};
    use crate::tools::registry::ToolRegistry;
    use crate::tools::traits::{ToolContext, ToolError, ToolExecutor, ToolOutput, ToolSpec};

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

    #[async_trait]
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

        async fn execute(
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
        registry.register(Arc::new(DummyTool::new())).unwrap();
        let registry = Arc::new(registry);

        let kernel = Kernel::new(Arc::clone(&registry));
        let tool = kernel.tool_registry().get("dummy").unwrap();
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(kernel.invoke_tool(tool.as_ref(), json!({})));

        assert!(result.is_err());
    }

    #[test]
    fn invoke_tool_allows_with_permission() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(DummyTool::new())).unwrap();
        let registry = Arc::new(registry);

        let mut capabilities = CapabilitySet::empty();
        capabilities.insert(Permission::FileRead {
            path: PathPattern("/tmp/allowed.txt".to_string()),
        });

        let kernel = Kernel::new(Arc::clone(&registry)).with_capabilities(capabilities);
        let tool = kernel.tool_registry().get("dummy").unwrap();
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(kernel.invoke_tool(tool.as_ref(), json!({})));

        assert!(result.is_ok());
    }
}
