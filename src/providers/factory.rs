use std::env;
use std::sync::Arc;

use anyhow::{Context, Result};
use rig::agent::Agent;
use rig::client::CompletionClient;
use rig::providers::openai;
use rig::tool::ToolDyn;

use crate::config::Config;
use crate::kernel::kernel::Kernel;
use crate::tools::registry::ToolRegistry;
use crate::tools::rig_wrapper::KernelBackedTool;

#[derive(Debug, Clone, Copy)]
pub enum ProviderKind {
    OpenAI,
}

impl ProviderKind {
    pub fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_lowercase().as_str() {
            "openai" => Ok(Self::OpenAI),
            other => Err(anyhow::anyhow!("unsupported provider '{other}'")),
        }
    }
}

pub struct ProviderFactory;

impl ProviderFactory {
    pub fn build_openai_client(config: &Config) -> Result<openai::Client> {
        let api_key_env = config.api_key_env.as_deref().unwrap_or("OPENAI_API_KEY");
        let api_key = env::var(api_key_env)
            .with_context(|| format!("missing API key in env '{api_key_env}'"))?;

        let mut builder = openai::Client::builder().api_key(api_key);
        if let Some(base_url) = &config.base_url {
            builder = builder.base_url(base_url);
        }

        builder.build().context("failed to build OpenAI client")
    }

    pub fn build_agent(
        config: &Config,
        tool_registry: &ToolRegistry,
        kernel: Arc<Kernel>,
    ) -> Result<Agent<openai::responses_api::ResponsesCompletionModel>> {
        let provider = ProviderKind::from_str(config.provider())?;

        match provider {
            ProviderKind::OpenAI => {
                let client = Self::build_openai_client(config)?;
                let builder = client
                    .agent(config.model())
                    .preamble(config.system_prompt())
                    .default_max_turns(config.max_turns());
                let tools = tool_registry
                    .specs()
                    .into_iter()
                    .map(|spec| {
                        let wrapped = KernelBackedTool::new(spec, kernel.clone());
                        Box::new(wrapped) as Box<dyn ToolDyn>
                    })
                    .collect::<Vec<_>>();

                if tools.is_empty() {
                    Ok(builder.build())
                } else {
                    Ok(builder.tools(tools).build())
                }
            }
        }
    }
}
