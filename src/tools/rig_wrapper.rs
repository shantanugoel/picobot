use std::sync::Arc;

use rig::completion::ToolDefinition;
use rig::tool::ToolDyn;
use rig::wasm_compat::WasmBoxedFuture;
use serde_json::Value;

use crate::kernel::core::Kernel;
use crate::tools::traits::ToolSpec;

#[derive(Clone)]
pub struct KernelBackedTool {
    spec: ToolSpec,
    kernel: Arc<Kernel>,
}

impl KernelBackedTool {
    pub fn new(spec: ToolSpec, kernel: Arc<Kernel>) -> Self {
        Self { spec, kernel }
    }
}

#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct KernelToolError(String);

impl ToolDyn for KernelBackedTool {
    fn name(&self) -> String {
        self.spec.name.clone()
    }

    fn definition<'a>(&'a self, _prompt: String) -> WasmBoxedFuture<'a, ToolDefinition> {
        Box::pin(async move {
            ToolDefinition {
                name: self.spec.name.clone(),
                description: self.spec.description.clone(),
                parameters: self.spec.schema.clone(),
            }
        })
    }

    fn call<'a>(
        &'a self,
        args: String,
    ) -> WasmBoxedFuture<'a, Result<String, rig::tool::ToolError>> {
        Box::pin(async move {
            let parsed: Value =
                serde_json::from_str(&args).map_err(rig::tool::ToolError::JsonError)?;
            let output = self
                .kernel
                .invoke_tool_with_prompt_by_name(&self.spec.name, parsed)
                .await
                .map_err(|err| rig::tool::ToolError::ToolCallError(Box::new(err)))?;
            serde_json::to_string(&output).map_err(rig::tool::ToolError::JsonError)
        })
    }
}
