use std::sync::Arc;

use serde_json::Value;

use crate::kernel::context::ToolContext;
use crate::kernel::permissions::CapabilitySet;
use crate::tools::registry::ToolRegistry;
use crate::tools::traits::{Tool, ToolError, ToolOutput};

pub struct Kernel {
    tool_registry: Arc<ToolRegistry>,
    context: ToolContext,
    memory_retriever: Option<crate::kernel::memory::MemoryRetriever>,
}

impl Kernel {
    pub fn new(tool_registry: ToolRegistry, working_dir: std::path::PathBuf) -> Self {
        Self {
            tool_registry: Arc::new(tool_registry),
            context: ToolContext {
                working_dir,
                capabilities: Arc::new(CapabilitySet::empty()),
                user_id: None,
                session_id: None,
                scheduler: Arc::new(std::sync::RwLock::new(None)),
                log_model_requests: false,
                include_tool_messages: false,
            },
            memory_retriever: None,
        }
    }

    pub fn with_capabilities(mut self, capabilities: CapabilitySet) -> Self {
        self.context.capabilities = Arc::new(capabilities);
        self
    }

    pub fn with_memory_retriever(mut self, memory: crate::kernel::memory::MemoryRetriever) -> Self {
        self.memory_retriever = Some(memory);
        self
    }

    pub fn clone_with_context(&self, user_id: Option<String>, session_id: Option<String>) -> Self {
        let mut context = self.context.clone();
        context.user_id = user_id;
        context.session_id = session_id;
        Self {
            tool_registry: Arc::clone(&self.tool_registry),
            context,
            memory_retriever: self.memory_retriever.clone(),
        }
    }

    pub fn tool_registry(&self) -> &ToolRegistry {
        self.tool_registry.as_ref()
    }

    pub fn context(&self) -> &ToolContext {
        &self.context
    }

    pub fn memory_retriever(&self) -> Option<&crate::kernel::memory::MemoryRetriever> {
        self.memory_retriever.as_ref()
    }

    pub fn set_working_dir(&mut self, working_dir: std::path::PathBuf) {
        self.context.working_dir = working_dir;
    }

    pub fn set_capabilities(&mut self, capabilities: CapabilitySet) {
        self.context.capabilities = Arc::new(capabilities);
    }

    pub fn set_scheduler(
        &mut self,
        scheduler: Option<Arc<crate::scheduler::service::SchedulerService>>,
    ) {
        if let Ok(mut slot) = self.context.scheduler.write() {
            *slot = scheduler;
        }
    }

    pub fn set_log_model_requests(&mut self, enabled: bool) {
        self.context.log_model_requests = enabled;
    }

    pub fn set_include_tool_messages(&mut self, enabled: bool) {
        self.context.include_tool_messages = enabled;
    }

    pub fn scheduler(&self) -> Option<Arc<crate::scheduler::service::SchedulerService>> {
        self.context.scheduler()
    }

    pub async fn invoke_tool(
        &self,
        tool: &dyn Tool,
        input: Value,
    ) -> Result<ToolOutput, ToolError> {
        self.invoke_tool_with_grants(tool, input, None).await
    }

    pub async fn invoke_tool_with_grants(
        &self,
        tool: &dyn Tool,
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
            return Err(ToolError::PermissionDenied {
                tool: tool.name().to_string(),
                required,
            });
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
    use async_trait::async_trait;
    use serde_json::json;

    use crate::kernel::agent::Kernel;
    use crate::kernel::context::ToolContext;
    use crate::kernel::permissions::{CapabilitySet, PathPattern, Permission};
    use crate::tools::registry::ToolRegistry;
    use crate::tools::traits::{Tool, ToolError, ToolOutput};

    #[derive(Debug)]
    struct DummyTool;

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &'static str {
            "dummy"
        }

        fn description(&self) -> &'static str {
            "dummy tool"
        }

        fn schema(&self) -> serde_json::Value {
            json!({"type": "object"})
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

    #[tokio::test]
    async fn invoke_tool_denies_without_permission() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DummyTool)).unwrap();

        let kernel = Kernel::new(registry, std::path::PathBuf::from("/"));
        let tool = kernel.tool_registry().get("dummy").unwrap();
        let result = kernel.invoke_tool(tool, json!({})).await;

        assert!(matches!(result, Err(ToolError::PermissionDenied { .. })));
    }

    #[tokio::test]
    async fn invoke_tool_allows_with_permission() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DummyTool)).unwrap();

        let mut capabilities = CapabilitySet::empty();
        capabilities.insert(Permission::FileRead {
            path: PathPattern("/tmp/allowed.txt".to_string()),
        });

        let kernel =
            Kernel::new(registry, std::path::PathBuf::from("/")).with_capabilities(capabilities);
        let tool = kernel.tool_registry().get("dummy").unwrap();
        let result = kernel.invoke_tool(tool, json!({})).await;

        assert!(result.is_ok());
    }
}
