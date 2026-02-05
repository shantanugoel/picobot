use std::collections::HashMap;
use std::sync::Arc;

use jsonschema::Validator;
use serde_json::Value;

use crate::kernel::permissions::Permission;
use crate::tools::traits::{ToolContext, ToolError, ToolExecutor, ToolSpec};

#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<Arc<dyn ToolExecutor>>,
    schemas: HashMap<String, Validator>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            schemas: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Arc<dyn ToolExecutor>) -> Result<(), ToolError> {
        let name = tool.spec().name.clone();
        let schema = tool.spec().schema.clone();
        let validator = jsonschema::validator_for(&schema)
            .map_err(|err| ToolError::new(format!("invalid schema for '{name}': {err}")))?;
        self.schemas.insert(name.clone(), validator);
        self.tools.push(tool);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn ToolExecutor>> {
        self.tools
            .iter()
            .find(|tool| tool.spec().name == name)
            .cloned()
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.iter().map(|tool| tool.spec().clone()).collect()
    }

    pub fn validate_input(&self, tool: &dyn ToolExecutor, input: &Value) -> Result<(), ToolError> {
        let name = &tool.spec().name;
        let validator = self
            .schemas
            .get(name)
            .ok_or_else(|| ToolError::new(format!("missing schema for '{name}'")))?;
        if validator.is_valid(input) {
            Ok(())
        } else {
            let errors = validator
                .iter_errors(input)
                .map(|err| err.to_string())
                .collect::<Vec<_>>()
                .join("; ");
            Err(ToolError::new(format!(
                "invalid input for '{name}': {errors}"
            )))
        }
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
