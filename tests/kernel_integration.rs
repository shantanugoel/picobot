use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use serde_json::json;

use picobot::kernel::core::Kernel;
use picobot::kernel::permissions::{
    CapabilitySet, ChannelPermissionProfile, PathPattern, Permission, PermissionPrompter,
    PromptDecision,
};
use picobot::tools::registry::ToolRegistry;
use picobot::tools::traits::{ToolContext, ToolError, ToolExecutor, ToolOutput, ToolSpec};

#[derive(Debug)]
struct StaticTool {
    spec: ToolSpec,
    required: Vec<Permission>,
}

impl StaticTool {
    fn new(name: &str, required: Vec<Permission>) -> Self {
        Self {
            spec: ToolSpec {
                name: name.to_string(),
                description: "static tool".to_string(),
                schema: json!({"type": "object"}),
            },
            required,
        }
    }
}

#[async_trait]
impl ToolExecutor for StaticTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn required_permissions(
        &self,
        _ctx: &ToolContext,
        _input: &serde_json::Value,
    ) -> Result<Vec<Permission>, ToolError> {
        Ok(self.required.clone())
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        _input: serde_json::Value,
    ) -> Result<ToolOutput, ToolError> {
        Ok(json!({"status": "ok"}))
    }
}

#[derive(Clone)]
struct MockPrompter {
    decision: Option<PromptDecision>,
    calls: Arc<AtomicUsize>,
}

impl MockPrompter {
    fn new(decision: Option<PromptDecision>) -> Self {
        Self {
            decision,
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl PermissionPrompter for MockPrompter {
    async fn prompt(
        &self,
        _tool_name: &str,
        _permissions: &[Permission],
        _timeout_secs: u64,
    ) -> Option<PromptDecision> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.decision
    }
}

fn read_permission() -> Permission {
    Permission::FileRead {
        path: PathPattern("/tmp/allowed.txt".to_string()),
    }
}

#[tokio::test]
async fn kernel_backed_prompt_flow_allow_once() {
    let required = vec![read_permission()];
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(StaticTool::new("dummy", required.clone())))
        .unwrap();
    let registry = Arc::new(registry);

    let mut profile = ChannelPermissionProfile::default();
    profile.max_allowed = CapabilitySet::from_permissions(&required);
    let prompter = Arc::new(MockPrompter::new(Some(PromptDecision::AllowOnce)));

    let kernel = Kernel::new(Arc::clone(&registry))
        .with_prompt_profile(profile)
        .with_prompter(Some(prompter));

    let output = kernel
        .invoke_tool_with_prompt_by_name("dummy", json!({}))
        .await;
    assert!(output.is_ok());
}

#[tokio::test]
async fn kernel_unknown_tool_returns_error() {
    let registry = Arc::new(ToolRegistry::new());
    let kernel = Kernel::new(registry);
    let result = kernel
        .invoke_tool_with_prompt_by_name("missing", json!({}))
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn kernel_denies_missing_permissions() {
    let required = vec![read_permission()];
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(StaticTool::new("dummy", required.clone())))
        .unwrap();
    let registry = Arc::new(registry);

    let kernel = Kernel::new(registry);
    let result = kernel
        .invoke_tool_with_prompt_by_name("dummy", json!({}))
        .await;
    assert!(result.is_err());
}
