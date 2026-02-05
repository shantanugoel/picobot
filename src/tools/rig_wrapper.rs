use std::sync::Arc;

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde_json::Value;

use crate::kernel::kernel::Kernel;
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

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct KernelToolError(String);

impl Tool for KernelBackedTool {
    const NAME: &'static str = "";

    type Error = KernelToolError;
    type Args = Value;
    type Output = Value;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: self.spec.name.clone(),
            description: self.spec.description.clone(),
            parameters: self.spec.schema.clone(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.kernel
            .invoke_tool_by_name(&self.spec.name, args)
            .map_err(|err| KernelToolError(err.to_string()))
    }
}
